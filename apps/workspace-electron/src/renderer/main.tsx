import React, { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';
import './globals.css';

type DaemonStatusResult = Awaited<ReturnType<typeof window.biohazardfs.daemonStatus>>;
type VersionInfo = Awaited<ReturnType<typeof window.biohazardfs.versions>>;
type DataRecord = Record<string, unknown>;
type Entry = Record<string, unknown>;
type ActionResult = { ok: boolean; text: string };

// The daemon response envelope is `{ ok, method, data, error, ... }`. The main
// process wraps fetch outcomes as `{ ok, endpoint, body?, error? }` where a
// missing `body` means the daemon was unreachable at the transport layer. Every
// helper below treats fields as untrusted draft data and falls back gracefully.

const DIRTY_STATES = new Set(['modified_local', 'uploading', 'dirty', 'offline_queued']);

function extractData(result: DaemonStatusResult | null): DataRecord | null {
  const body = result?.body as { data?: DataRecord } | undefined;
  return body?.data ?? null;
}

function hasBody(result: DaemonStatusResult | null): boolean {
  return result?.body !== undefined;
}

function extractError(result: DaemonStatusResult | null): { code: string; message: string } | null {
  if (!result) {
    return null;
  }
  const body = result.body as { error?: { code?: string; message?: string } } | undefined;
  const error = body?.error;
  if (error && typeof error.code === 'string') {
    return {
      code: error.code,
      message: typeof error.message === 'string' ? error.message : 'unknown error',
    };
  }
  if (typeof result.error === 'string' && result.error.length > 0) {
    return { code: 'unreachable', message: result.error };
  }
  return null;
}

function asNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) {
    return value;
  }
  if (typeof value === 'string') {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) {
      return parsed;
    }
  }
  return null;
}

function asString(value: unknown, fallback = ''): string {
  if (typeof value === 'string' && value.length > 0) {
    return value;
  }
  return fallback;
}

function entryList(data: DataRecord | null, fields: string[]): Entry[] {
  for (const field of fields) {
    const value = data?.[field];
    if (Array.isArray(value)) {
      return value.filter((item): item is Entry => typeof item === 'object' && item !== null);
    }
  }
  return [];
}

function formatBytes(value: unknown): string {
  const bytes = asNumber(value);
  if (bytes === null || bytes < 0) {
    return 'unknown';
  }
  if (bytes < 1024) {
    return `${String(Math.round(bytes))} B`;
  }
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let scaled = bytes / 1024;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2;
  return `${scaled.toFixed(digits)} ${units[index]}`;
}

function isDirtyEntry(entry: Entry): boolean {
  if (entry.dirty === true) {
    return true;
  }
  return DIRTY_STATES.has(asString(entry.state));
}

function isPinnedEntry(entry: Entry): boolean {
  if (entry.pinned === true) {
    return true;
  }
  const state = asString(entry.state);
  return state === 'pinned' || state === 'cached_pinned';
}

// When the daemon is unreachable, keep the last good card data visible so the
// artist is not staring at empty panels during a transient dropout.
function keepLastGood(
  previous: DaemonStatusResult | null,
  next: DaemonStatusResult,
): DaemonStatusResult {
  if (next.body !== undefined) {
    return next;
  }
  return previous ?? next;
}

function directionLabel(direction: unknown): string {
  if (direction === 'upload') {
    return 'Uploading';
  }
  if (direction === 'download') {
    return 'Downloading';
  }
  return 'Syncing';
}

function stateLabel(state: unknown): string {
  const value = asString(state);
  switch (value) {
    case 'running':
      return 'Syncing';
    case 'queued':
    case 'pending':
      return 'Queued';
    case 'completed':
    case 'done':
    case 'synced':
      return 'Done';
    case 'failed':
    case 'error':
      return 'Failed';
    case 'paused':
      return 'Paused';
    case 'cancelled':
    case 'canceled':
      return 'Cancelled';
    case '':
      return 'Queued';
    default:
      return value;
  }
}

function computeProgress(entry: Entry): { percent: number; label: string } {
  const direct = asNumber(entry.progress);
  if (direct !== null && direct >= 0 && direct <= 1) {
    const percent = Math.round(direct * 100);
    return { percent, label: `${String(percent)}%` };
  }
  if (direct !== null && direct > 1 && direct <= 100) {
    const percent = Math.round(direct);
    return { percent, label: `${String(percent)}%` };
  }
  const done = asNumber(entry.bytes_done);
  const total = asNumber(entry.bytes_total);
  if (done !== null && total !== null && total > 0) {
    const ratio = Math.min(Math.max(done / total, 0), 1);
    const percent = Math.round(ratio * 100);
    return { percent, label: `${String(percent)}%` };
  }
  return { percent: 0, label: '—' };
}

function Pill({
  tone = 'muted',
  children,
}: {
  tone?: 'good' | 'warn' | 'muted';
  children: React.ReactNode;
}) {
  return <span className={`pill ${tone}`}>{children}</span>;
}

function EmptyState({ children }: { children: React.ReactNode }) {
  return <p className="note">{children}</p>;
}

function ErrorNote({ label, error }: { label: string; error: { code: string; message: string } }) {
  return (
    <p className="note error">
      <strong>{label}.</strong> <span className="muted-text">{error.message}</span>
      <code className="error-code">{error.code}</code>
    </p>
  );
}

function Feedback({ result }: { result: ActionResult }) {
  return <p className={result.ok ? 'note good' : 'note warn'}>{result.text}</p>;
}

function DaemonCard({
  result,
  versions,
}: {
  result: DaemonStatusResult | null;
  versions: VersionInfo | null;
}) {
  const reachable = hasBody(result);
  const error = extractError(result);
  const statusText = reachable
    ? result?.ok
      ? 'ready'
      : (error?.code ?? 'error')
    : (error?.message ?? 'not checked yet');
  return (
    <article className="card">
      <div className="card-header">
        <h2>Daemon</h2>
        {reachable ? <Pill tone="good">Connected</Pill> : <Pill tone="warn">Waiting</Pill>}
      </div>
      <dl>
        <dt>Endpoint</dt>
        <dd>{result?.endpoint ?? '127.0.0.1:47666'}</dd>
        <dt>Status</dt>
        <dd>{statusText}</dd>
        {versions ? (
          <>
            <dt>App</dt>
            <dd>{`v${versions.app}`}</dd>
          </>
        ) : null}
      </dl>
    </article>
  );
}

function WorkspaceCard({
  result,
  listResult,
  ready,
}: {
  result: DaemonStatusResult | null;
  listResult: DaemonStatusResult | null;
  ready: boolean;
}) {
  const data = extractData(result);
  const error = extractError(result);
  const entries = entryList(data, ['entries']);
  const loading = result === null;
  const stateText = asString(data?.state) || error?.message || 'unknown';
  return (
    <article className="card">
      <div className="card-header">
        <h2>Workspace</h2>
        {ready ? <Pill tone="good">Visible</Pill> : <Pill tone="warn">Not configured</Pill>}
      </div>
      {loading ? (
        <EmptyState>Checking workspace…</EmptyState>
      ) : (
        <>
          <dl>
            <dt>Root</dt>
            <dd>{asString(data?.root) || 'not configured'}</dd>
            <dt>State</dt>
            <dd>{stateText}</dd>
          </dl>
          <ul className="checklist">
            {entries.length > 0 ? (
              entries.map((entry, index) => (
                <li key={`ws:${String(index)}:${asString(entry.name)}`}>
                  {asString(entry.name)} <span className="muted-text">{asString(entry.kind)}</span>
                </li>
              ))
            ) : (
              <li>{listResult?.ok ? 'Workspace root is empty' : 'Workspace list unavailable'}</li>
            )}
          </ul>
        </>
      )}
    </article>
  );
}

function CacheCard({
  status,
  list,
  onPin,
  onDehydrate,
}: {
  status: DaemonStatusResult | null;
  list: DaemonStatusResult | null;
  onPin: (path: string) => Promise<DaemonStatusResult>;
  onDehydrate: (path: string) => Promise<DaemonStatusResult>;
}) {
  const [pendingPath, setPendingPath] = useState<string | null>(null);
  const [feedback, setFeedback] = useState<ActionResult | null>(null);

  const statusData = extractData(status);
  const listData = extractData(list);
  const entries = entryList(listData, ['entries', 'items']);
  const usedBytes = asNumber(statusData?.used_bytes ?? statusData?.bytes_used);
  const pinnedBytes = asNumber(statusData?.pinned_bytes);
  const dirtyBytes = asNumber(statusData?.dirty_bytes);
  const dirtyCount = asNumber(statusData?.dirty_count);
  const statusError = extractError(status);
  const listError = extractError(list);
  const loading = status === null && list === null;
  const dirtySignal = dirtyCount !== null && dirtyCount > 0;

  async function act(kind: 'pin' | 'dehydrate', entry: Entry) {
    const path = asString(entry.path);
    if (!path) {
      setFeedback({ ok: false, text: 'This entry has no path to act on.' });
      return;
    }
    setFeedback(null);
    if (kind === 'dehydrate' && isDirtyEntry(entry)) {
      // Safety invariant: never remove a local copy with unsynced changes.
      setFeedback({
        ok: false,
        text: 'Local changes have not synced yet. Keeping the local copy.',
      });
      return;
    }
    setPendingPath(path);
    const result = kind === 'pin' ? await onPin(path) : await onDehydrate(path);
    setPendingPath(null);
    const actionError = extractError(result);
    if (actionError) {
      setFeedback({
        ok: false,
        text: `${kind === 'pin' ? 'Make available offline' : 'Remove local copy'} could not complete (${actionError.code}).`,
      });
    } else {
      setFeedback({
        ok: true,
        text: kind === 'pin' ? 'Marked to stay available offline.' : 'Local copy removed.',
      });
    }
  }

  return (
    <article className="card">
      <div className="card-header">
        <h2>Local cache</h2>
        {dirtySignal ? (
          <Pill tone="warn">{`${String(dirtyCount)} waiting to sync`}</Pill>
        ) : status?.ok ? (
          <Pill tone="good">In sync</Pill>
        ) : (
          <Pill tone="muted">Unavailable</Pill>
        )}
      </div>
      {loading ? (
        <EmptyState>Checking local cache…</EmptyState>
      ) : statusError && !statusData ? (
        <ErrorNote label="Cache status unavailable" error={statusError} />
      ) : (
        <>
          <dl>
            <dt>Used</dt>
            <dd>{usedBytes !== null ? formatBytes(usedBytes) : 'unknown'}</dd>
            <dt>Available offline</dt>
            <dd>{pinnedBytes !== null ? formatBytes(pinnedBytes) : 'unknown'}</dd>
            <dt>Waiting to sync</dt>
            <dd>{dirtyBytes !== null ? formatBytes(dirtyBytes) : 'unknown'}</dd>
          </dl>
          {dirtySignal ? (
            <p className="note warn">
              {`${String(dirtyCount)} file(s) have local changes that have not synced. They will not be removed.`}
            </p>
          ) : null}
          {feedback ? <Feedback result={feedback} /> : null}
          <ul className="entry-list">
            {entries.length === 0 ? (
              <li className="muted-text">
                {listError
                  ? `Cache list unavailable (${listError.code})`
                  : 'No cached files shown.'}
              </li>
            ) : (
              entries.map((entry, index) => {
                const path = asString(entry.path, `(entry ${String(index + 1)})`);
                const dirty = isDirtyEntry(entry);
                const pinned = isPinnedEntry(entry);
                const busy = pendingPath === path;
                const size = entry.size_bytes;
                return (
                  <li key={`cache:${String(index)}:${path}`} className="entry">
                    <div className="entry-main">
                      <span className="entry-path">{path}</span>
                      <span className="entry-tags">
                        {pinned ? <span className="tag pinned">Available offline</span> : null}
                        {dirty ? <span className="tag dirty">Not synced</span> : null}
                        {size !== undefined && size !== null ? (
                          <span className="muted-text">{formatBytes(size)}</span>
                        ) : null}
                      </span>
                    </div>
                    <div className="entry-controls">
                      <button
                        type="button"
                        className="btn-sm"
                        disabled={busy}
                        onClick={() => void act('pin', entry)}
                      >
                        {busy ? 'Working…' : 'Make available offline'}
                      </button>
                      <button
                        type="button"
                        className="btn-sm ghost"
                        disabled={dirty || busy}
                        title={dirty ? 'Local changes have not synced yet' : undefined}
                        onClick={() => void act('dehydrate', entry)}
                      >
                        Remove local copy
                      </button>
                    </div>
                  </li>
                );
              })
            )}
          </ul>
        </>
      )}
    </article>
  );
}

function TransferCard({
  list,
  onPause,
  onResume,
}: {
  list: DaemonStatusResult | null;
  onPause: () => Promise<DaemonStatusResult>;
  onResume: () => Promise<DaemonStatusResult>;
}) {
  const [busy, setBusy] = useState<'pause' | 'resume' | null>(null);
  const [feedback, setFeedback] = useState<ActionResult | null>(null);

  const data = extractData(list);
  const transfers = entryList(data, ['entries', 'transfers', 'items']);
  const error = extractError(list);
  const loading = list === null;

  async function run(kind: 'pause' | 'resume') {
    setFeedback(null);
    setBusy(kind);
    const result = kind === 'pause' ? await onPause() : await onResume();
    setBusy(null);
    const runError = extractError(result);
    if (runError) {
      setFeedback({
        ok: false,
        text: `${kind === 'pause' ? 'Pause' : 'Resume'} unavailable (${runError.code}).`,
      });
    } else {
      setFeedback({
        ok: true,
        text: kind === 'pause' ? 'Transfers paused.' : 'Transfers resumed.',
      });
    }
  }

  return (
    <article className="card">
      <div className="card-header">
        <h2>Sync activity</h2>
        {error ? (
          <Pill tone="muted">Unavailable</Pill>
        ) : transfers.length > 0 ? (
          <Pill tone="good">Live</Pill>
        ) : (
          <Pill tone="muted">Idle</Pill>
        )}
      </div>
      {loading ? (
        <EmptyState>Checking sync activity…</EmptyState>
      ) : error && transfers.length === 0 ? (
        <ErrorNote label="Transfer list unavailable" error={error} />
      ) : (
        <>
          <div className="entry-controls inline">
            <button
              type="button"
              className="btn-sm ghost"
              disabled={busy !== null}
              onClick={() => void run('pause')}
            >
              {busy === 'pause' ? 'Pausing…' : 'Pause all'}
            </button>
            <button
              type="button"
              className="btn-sm ghost"
              disabled={busy !== null}
              onClick={() => void run('resume')}
            >
              {busy === 'resume' ? 'Resuming…' : 'Resume all'}
            </button>
          </div>
          {feedback ? <Feedback result={feedback} /> : null}
          <ul className="entry-list">
            {transfers.length === 0 ? (
              <li className="muted-text">Nothing transferring right now.</li>
            ) : (
              transfers.map((transfer, index) => {
                const id = asString(
                  transfer.id ?? transfer.transfer_id,
                  `transfer-${String(index)}`,
                );
                const direction = directionLabel(transfer.direction);
                const state = stateLabel(transfer.state);
                const path = asString(transfer.path, 'unknown path');
                const progress = computeProgress(transfer);
                const directionClass = transfer.direction === 'upload' ? 'upload' : 'download';
                return (
                  <li key={`transfer:${String(index)}:${id}`} className="entry">
                    <div className="entry-main">
                      <span className="entry-path">{path}</span>
                      <span className="entry-tags">
                        <span className={`tag ${directionClass}`}>{direction}</span>
                        <span className="muted-text">{state}</span>
                      </span>
                    </div>
                    <div className="progress" aria-hidden="true">
                      <div
                        className="progress-bar"
                        style={{ width: `${String(progress.percent)}%` }}
                      />
                      <span className="progress-label">{progress.label}</span>
                    </div>
                  </li>
                );
              })
            )}
          </ul>
        </>
      )}
    </article>
  );
}

function ConflictCard({
  list,
  onPreserveAll,
}: {
  list: DaemonStatusResult | null;
  onPreserveAll: () => Promise<DaemonStatusResult>;
}) {
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<ActionResult | null>(null);

  const data = extractData(list);
  const conflicts = entryList(data, ['entries', 'conflicts', 'items']);
  const error = extractError(list);
  const loading = list === null;

  async function preserveAll() {
    setFeedback(null);
    setBusy(true);
    const result = await onPreserveAll();
    setBusy(false);
    const preserveError = extractError(result);
    if (preserveError) {
      setFeedback({ ok: false, text: `Preserve all unavailable (${preserveError.code}).` });
    } else {
      setFeedback({ ok: true, text: 'All versions preserved.' });
    }
  }

  return (
    <article className="card">
      <div className="card-header">
        <h2>Conflicts</h2>
        {error ? (
          <Pill tone="muted">Unavailable</Pill>
        ) : conflicts.length > 0 ? (
          <Pill tone="warn">{`${String(conflicts.length)} open`}</Pill>
        ) : (
          <Pill tone="good">None</Pill>
        )}
      </div>
      {loading ? (
        <EmptyState>Checking for conflicts…</EmptyState>
      ) : error && conflicts.length === 0 ? (
        <ErrorNote label="Conflict list unavailable" error={error} />
      ) : conflicts.length === 0 ? (
        <EmptyState>No conflicts. Divergent work is always preserved separately.</EmptyState>
      ) : (
        <>
          <ul className="entry-list">
            {conflicts.map((conflict, index) => {
              const id = asString(conflict.id ?? conflict.path, `conflict-${String(index)}`);
              const path = asString(conflict.path, 'unknown path');
              const actor = asString(conflict.actor);
              const timestamp = asString(conflict.timestamp);
              return (
                <li key={`conflict:${String(index)}:${id}`} className="entry">
                  <div className="entry-main">
                    <span className="entry-path">{path}</span>
                    <span className="entry-tags">
                      {conflict.kind ? (
                        <span className="tag">{asString(conflict.kind)}</span>
                      ) : null}
                      {conflict.status ? (
                        <span className="muted-text">{asString(conflict.status)}</span>
                      ) : null}
                    </span>
                  </div>
                  <div className="entry-meta">
                    {actor ? <span className="muted-text">{`by ${actor}`}</span> : null}
                    {timestamp ? <span className="muted-text">{timestamp}</span> : null}
                  </div>
                </li>
              );
            })}
          </ul>
          <button
            type="button"
            className="btn-sm"
            disabled={busy}
            onClick={() => void preserveAll()}
          >
            {busy ? 'Preserving…' : 'Preserve all versions'}
          </button>
          {feedback ? <Feedback result={feedback} /> : null}
        </>
      )}
    </article>
  );
}

function LockCard({ list }: { list: DaemonStatusResult | null }) {
  const data = extractData(list);
  const locks = entryList(data, ['entries', 'locks', 'items']);
  const error = extractError(list);
  const loading = list === null;

  return (
    <article className="card">
      <div className="card-header">
        <h2>Locks</h2>
        {error ? (
          <Pill tone="muted">Unavailable</Pill>
        ) : locks.length > 0 ? (
          <Pill tone="warn">{`${String(locks.length)} held`}</Pill>
        ) : (
          <Pill tone="good">None</Pill>
        )}
      </div>
      {loading ? (
        <EmptyState>Checking locks…</EmptyState>
      ) : error && locks.length === 0 ? (
        <ErrorNote label="Lock list unavailable" error={error} />
      ) : locks.length === 0 ? (
        <EmptyState>
          No locks held. Scene and binary files can be locked to block conflicting edits.
        </EmptyState>
      ) : (
        <ul className="entry-list">
          {locks.map((lock, index) => {
            const key = asString(lock.path, `lock-${String(index)}`);
            const path = asString(lock.path, 'unknown path');
            const owner = asString(lock.owner);
            // If a lock entry exists but the boolean is missing, default to
            // locked: never falsely advertise that a file is safe to edit.
            const locked = lock.locked !== false && asString(lock.state) !== 'unlocked';
            const expires = asString(lock.expires ?? lock.expires_at);
            return (
              <li
                key={`lock:${String(index)}:${key}`}
                className={`entry ${locked ? 'is-locked' : 'is-unlocked'}`}
              >
                <div className="entry-main">
                  <span className="entry-path">{path}</span>
                  <span className="entry-tags">
                    {locked ? (
                      <span className="tag locked">{owner ? `Locked by ${owner}` : 'Locked'}</span>
                    ) : (
                      <span className="tag unlocked">Unlocked</span>
                    )}
                    {lock.kind ? <span className="muted-text">{asString(lock.kind)}</span> : null}
                  </span>
                </div>
                {expires ? (
                  <div className="entry-meta">
                    <span className="muted-text">{`expires ${expires}`}</span>
                  </div>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </article>
  );
}

function OnboardingCard({ onSave }: { onSave: (path: string) => Promise<DaemonStatusResult> }) {
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<ActionResult | null>(null);

  async function submit() {
    const trimmed = path.trim();
    if (!trimmed) {
      setFeedback({ ok: false, text: 'Enter a folder path first.' });
      return;
    }
    setFeedback(null);
    setBusy(true);
    const result = await onSave(trimmed);
    setBusy(false);
    const error = extractError(result);
    if (error) {
      setFeedback({ ok: false, text: `Could not save cache location yet (${error.code}).` });
    } else {
      setFeedback({ ok: true, text: 'Cache location saved.' });
    }
  }

  return (
    <article className="card onboarding">
      <div className="card-header">
        <h2>Set up local storage</h2>
        <Pill tone="warn">Setup needed</Pill>
      </div>
      <p className="note">
        Choose where BiohazardFS keeps local copies of your files. This is a scaffold — full
        onboarding lands later.
      </p>
      <form
        className="inline-form"
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <input
          className="input"
          type="text"
          placeholder="/Volumes/Work/biohazardfs-cache"
          value={path}
          onChange={(event) => {
            setPath(event.target.value);
          }}
          aria-label="Cache location"
        />
        <button type="submit" className="btn-sm" disabled={busy}>
          {busy ? 'Saving…' : 'Save cache location'}
        </button>
      </form>
      {feedback ? <Feedback result={feedback} /> : null}
    </article>
  );
}

function App() {
  const [daemon, setDaemon] = useState<DaemonStatusResult | null>(null);
  const [workspace, setWorkspace] = useState<DaemonStatusResult | null>(null);
  const [workspaceList, setWorkspaceList] = useState<DaemonStatusResult | null>(null);
  const [cacheStatus, setCacheStatus] = useState<DaemonStatusResult | null>(null);
  const [cacheList, setCacheList] = useState<DaemonStatusResult | null>(null);
  const [transferList, setTransferList] = useState<DaemonStatusResult | null>(null);
  const [conflictList, setConflictList] = useState<DaemonStatusResult | null>(null);
  const [lockList, setLockList] = useState<DaemonStatusResult | null>(null);
  const [versions, setVersions] = useState<VersionInfo | null>(null);
  const [daemonReachable, setDaemonReachable] = useState<boolean | null>(null);
  const [loaded, setLoaded] = useState(false);

  const fetchAll = useCallback(async () => {
    const [
      daemonRes,
      workspaceRes,
      listRes,
      cacheStatusRes,
      cacheListRes,
      transferRes,
      conflictRes,
      lockRes,
      versionInfo,
    ] = await Promise.all([
      window.biohazardfs.daemonStatus(),
      window.biohazardfs.workspaceStatus(),
      window.biohazardfs.workspaceList(''),
      window.biohazardfs.cacheStatus(),
      window.biohazardfs.cacheList(),
      window.biohazardfs.transferList(),
      window.biohazardfs.conflictList(),
      window.biohazardfs.lockList(),
      window.biohazardfs.versions(),
    ]);
    return {
      daemon: daemonRes,
      workspace: workspaceRes,
      workspaceList: listRes,
      cacheStatus: cacheStatusRes,
      cacheList: cacheListRes,
      transferList: transferRes,
      conflictList: conflictRes,
      lockList: lockRes,
      versions: versionInfo,
    };
  }, []);

  const applyResults = useCallback((results: Awaited<ReturnType<typeof fetchAll>>) => {
    setDaemonReachable(results.daemon.body !== undefined);
    setDaemon(results.daemon);
    setVersions(results.versions);
    setWorkspace((prev) => keepLastGood(prev, results.workspace));
    setWorkspaceList((prev) => keepLastGood(prev, results.workspaceList));
    setCacheStatus((prev) => keepLastGood(prev, results.cacheStatus));
    setCacheList((prev) => keepLastGood(prev, results.cacheList));
    setTransferList((prev) => keepLastGood(prev, results.transferList));
    setConflictList((prev) => keepLastGood(prev, results.conflictList));
    setLockList((prev) => keepLastGood(prev, results.lockList));
    setLoaded(true);
  }, []);

  useEffect(() => {
    let cancelled = false;
    void fetchAll().then((results) => {
      if (cancelled) {
        return;
      }
      applyResults(results);
    });
    return () => {
      cancelled = true;
    };
  }, [fetchAll, applyResults]);

  const refreshAll = useCallback(async () => {
    applyResults(await fetchAll());
  }, [applyResults, fetchAll]);

  const refreshCache = useCallback(async () => {
    const [status, list] = await Promise.all([
      window.biohazardfs.cacheStatus(),
      window.biohazardfs.cacheList(),
    ]);
    setCacheStatus((prev) => keepLastGood(prev, status));
    setCacheList((prev) => keepLastGood(prev, list));
  }, []);

  const pinEntry = useCallback(
    async (path: string) => {
      const result = await window.biohazardfs.cachePin({ path });
      await refreshCache();
      return result;
    },
    [refreshCache],
  );

  const dehydrateEntry = useCallback(
    async (path: string) => {
      const result = await window.biohazardfs.cacheDehydrate({ path });
      await refreshCache();
      return result;
    },
    [refreshCache],
  );

  const pauseTransfers = useCallback(async () => {
    const result = await window.biohazardfs.transferPause({});
    const list = await window.biohazardfs.transferList();
    setTransferList((prev) => keepLastGood(prev, list));
    return result;
  }, []);

  const resumeTransfers = useCallback(async () => {
    const result = await window.biohazardfs.transferResume({});
    const list = await window.biohazardfs.transferList();
    setTransferList((prev) => keepLastGood(prev, list));
    return result;
  }, []);

  const preserveAllConflicts = useCallback(async () => {
    const result = await window.biohazardfs.conflictPreserveAll();
    const list = await window.biohazardfs.conflictList();
    setConflictList((prev) => keepLastGood(prev, list));
    return result;
  }, []);

  const saveCacheLocation = useCallback(async (path: string) => {
    return window.biohazardfs.configSet({ key: 'cache.path', value: path });
  }, []);

  const workspaceData = extractData(workspace);
  const workspaceReady = workspaceData?.state === 'ready';
  const showOnboarding = loaded && workspaceData?.state !== 'ready';
  const showOfflineBanner = loaded && daemonReachable === false;

  return (
    <main className="app-shell">
      <section className="hero-panel">
        <p className="eyebrow">BiohazardFS client scaffold</p>
        <h1>Biohazard Workspace</h1>
        <p className="lede">
          Desktop shell reading daemon state. Not a production sync client yet.
        </p>
        <div className="hero-actions">
          <button type="button" onClick={() => void refreshAll()}>
            Refresh
          </button>
        </div>
      </section>

      {showOfflineBanner ? (
        <div className="banner offline" role="status">
          Daemon offline — showing last known state.
        </div>
      ) : null}

      {showOnboarding ? <OnboardingCard onSave={saveCacheLocation} /> : null}

      <section className="grid">
        <DaemonCard result={daemon} versions={versions} />
        <WorkspaceCard result={workspace} listResult={workspaceList} ready={workspaceReady} />
        <CacheCard
          status={cacheStatus}
          list={cacheList}
          onPin={pinEntry}
          onDehydrate={dehydrateEntry}
        />
        <TransferCard list={transferList} onPause={pauseTransfers} onResume={resumeTransfers} />
        <ConflictCard list={conflictList} onPreserveAll={preserveAllConflicts} />
        <LockCard list={lockList} />
        <article className="card wide diagnostics">
          <div className="card-header">
            <h2>Diagnostics</h2>
            <Pill tone="muted">Raw</Pill>
          </div>
          <p className="note">
            Raw daemon envelopes for development. Field names are draft and may be missing.
          </p>
          <pre>
            {JSON.stringify(
              {
                versions,
                daemon,
                workspace,
                workspaceList,
                cacheStatus,
                cacheList,
                transferList,
                conflictList,
                lockList,
              },
              null,
              2,
            )}
          </pre>
        </article>
      </section>
    </main>
  );
}

const root = document.getElementById('root');
if (!root) {
  throw new Error('missing root element');
}

createRoot(root).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
