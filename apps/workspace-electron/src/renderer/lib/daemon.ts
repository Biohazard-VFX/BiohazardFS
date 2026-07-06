// Defensive parsing of daemon response envelopes.
//
// The daemon response envelope is `{ ok, method, data, error, ... }`. The main
// process wraps fetch outcomes as `{ ok, endpoint, body?, error? }` where a
// missing `body` means the daemon was unreachable at the transport layer. Every
// helper below treats fields as untrusted draft data and falls back gracefully.
//
// SAFETY: several of these encode filesystem/sync invariants. Do not relax them
// without a matching test in `__tests__/daemon.test.ts`:
//   - isDirtyEntry: a file with unsynced local changes. Drives the
//     "Remove local copy" disable + the clear-all-local-cache refuse path.
//     Never silently return false for a dirty file.
//   - keepLastGood: keep the last good card data during a transient dropout.
//
// These were lifted verbatim from the original `main.tsx` renderer. Behavior is
// unchanged; they were relocated to be unit-testable and reused across views.

export type DaemonStatusResult = Awaited<ReturnType<typeof window.biohazardfs.daemonStatus>>;
export type VersionInfo = Awaited<ReturnType<typeof window.biohazardfs.versions>>;
export type DataRecord = Record<string, unknown>;
export type Entry = Record<string, unknown>;

// Entry states that mean "local changes have not been synced." Mirror the
// daemon's own dirty-state vocabulary. Add here when the daemon adds a new
// dirty state, never by removing one.
export const DIRTY_STATES: ReadonlySet<string> = new Set([
  'modified_local',
  'uploading',
  'dirty',
  'offline_queued',
]);

export function extractData(result: DaemonStatusResult | null): DataRecord | null {
  const body = result?.body as { data?: DataRecord } | undefined;
  return body?.data ?? null;
}

export function hasBody(result: DaemonStatusResult | null): boolean {
  return result?.body !== undefined;
}

export function extractError(result: DaemonStatusResult | null): {
  code: string;
  message: string;
} | null {
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

export function asNumber(value: unknown): number | null {
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

export function asString(value: unknown, fallback = ''): string {
  if (typeof value === 'string' && value.length > 0) {
    return value;
  }
  return fallback;
}

export function entryList(data: DataRecord | null, fields: string[]): Entry[] {
  for (const field of fields) {
    const value = data?.[field];
    if (Array.isArray(value)) {
      return value.filter((item): item is Entry => typeof item === 'object' && item !== null);
    }
  }
  return [];
}

export function isDirtyEntry(entry: Entry): boolean {
  if (entry.dirty === true) {
    return true;
  }
  return DIRTY_STATES.has(asString(entry.state));
}

export function isPinnedEntry(entry: Entry): boolean {
  if (entry.pinned === true) {
    return true;
  }
  const state = asString(entry.state);
  return state === 'pinned' || state === 'cached_pinned';
}

// When the daemon is unreachable, keep the last good card data visible so the
// artist is not staring at empty panels during a transient dropout. A missing
// `body` means transport-level failure; otherwise trust the new envelope, even
// if it carries a daemon-level error (the error is rendered inline).
export function keepLastGood(
  previous: DaemonStatusResult | null,
  next: DaemonStatusResult | null,
): DaemonStatusResult | null {
  if (next && next.body !== undefined) {
    return next;
  }
  return previous ?? next;
}

export function directionLabel(direction: unknown): string {
  if (direction === 'upload') {
    return 'Uploading';
  }
  if (direction === 'download') {
    return 'Downloading';
  }
  return 'Syncing';
}

export function stateLabel(state: unknown): string {
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

export function computeProgress(entry: Entry): { percent: number; label: string } {
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
