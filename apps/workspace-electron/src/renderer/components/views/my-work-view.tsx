import {
  AlertTriangle,
  ArrowDownToLine,
  ArrowUpFromLine,
  CheckCircle2,
  FolderOpen,
  HardDrive,
  Lock,
  ShieldAlert,
  ShieldCheck,
} from 'lucide-react';

import { type DaemonSnapshot } from '@/lib/use-daemon';
import { useDaemonFetch } from '@/lib/use-fetch';
import {
  type Entry,
  asNumber,
  asString,
  dirtyEntryCount,
  entryList,
  extractData,
  mountAttached,
  mountPathFromList,
} from '@/lib/daemon';
import { formatBytes } from '@/lib/format';
import { cn } from '@/lib/utils';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { ScrollArea } from '@/components/ui/scroll-area';
import { ViewLoading } from '@/components/view-states';

// My Work — the default landing page. Answers the five questions from
// DASHBOARD_UX §4.3: can I work, where is my work, is it synced, what's
// offline, what's dangerous. Reads cross-cutting state from the global
// snapshot; fetches workset.list + mount.status lazily (they're not in the
// always-polled set).
type Props = {
  snapshot: DaemonSnapshot;
  loaded: boolean;
  onOpenDrive: () => void;
  refreshNonce: number;
};

export function MyWorkView({ snapshot, loaded, onOpenDrive, refreshNonce }: Props) {
  const cacheStatus = extractData(snapshot.cacheStatus);
  const transferData = extractData(snapshot.transferList);
  const conflictData = extractData(snapshot.conflictList);
  const lockData = extractData(snapshot.lockList);
  const workspaceData = extractData(snapshot.workspace);

  const reachable = snapshot.daemon?.body !== undefined;
  const workspaceReady = workspaceData?.state === 'ready';

  const dirtyCount = dirtyEntryCount(cacheStatus) ?? 0;
  const dirtyBytes = asNumber(cacheStatus?.dirty_bytes);
  const usedBytes = asNumber(cacheStatus?.used_bytes ?? cacheStatus?.bytes_used);
  const pinnedBytes = asNumber(cacheStatus?.pinned_bytes);
  const conflictCount = entryList(conflictData, ['entries', 'conflicts', 'items']).length;
  const lockCount = entryList(lockData, ['entries', 'locks', 'items']).length;
  const transfers = entryList(transferData, ['entries', 'transfers', 'items']);
  const activeUploads = transfers.filter(
    (t) => asString(t.direction) === 'upload' && isActive(t),
  ).length;
  const activeDownloads = transfers.filter(
    (t) => asString(t.direction) === 'download' && isActive(t),
  ).length;
  const failedTransfers = transfers.filter((t) => isFailed(t)).length;

  const worksets = useDaemonFetch('workset.list', {}, refreshNonce);
  const mountStatusData = extractData(snapshot.mountStatus);
  const mountListData = extractData(snapshot.mountList);
  const worksetEntries = entryList(worksets.data, ['entries', 'worksets', 'items']);
  const mounted = mountAttached(mountStatusData);
  const mountPath = mountPathFromList(mountListData);

  if (!loaded) return <ViewLoading rows={6} />;

  const safeToEdit =
    reachable && workspaceReady && mounted && dirtyCount === 0 && failedTransfers === 0;
  const safeToQuit = dirtyCount === 0 && activeUploads === 0;
  const hasAttention = !reachable || dirtyCount > 0 || conflictCount > 0 || failedTransfers > 0;

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex max-w-4xl flex-col gap-4 p-5">
        {/* Readiness banner */}
        <ReadinessBanner
          reachable={reachable}
          mounted={mounted}
          safeToEdit={safeToEdit}
          mountPath={mountPath}
          onOpenDrive={onOpenDrive}
        />

        {/* Attention required */}
        {hasAttention ? (
          <AttentionCard
            reachable={reachable}
            dirtyCount={dirtyCount}
            dirtyBytes={dirtyBytes}
            conflictCount={conflictCount}
            lockCount={lockCount}
            failedTransfers={failedTransfers}
          />
        ) : null}

        {/* Available workspaces */}
        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Available workspaces</CardTitle>
          </CardHeader>
          <CardContent>
            {worksets.loading ? (
              <p className="text-muted-foreground text-xs">Loading…</p>
            ) : worksetEntries.length === 0 ? (
              <p className="text-muted-foreground text-xs">
                {workspaceReady
                  ? 'No workspaces exposed by workset.list yet.'
                  : 'No workspace mounted. Connect a studio to see your work.'}
              </p>
            ) : (
              <ul className="flex flex-col gap-1.5">
                {worksetEntries.map((w, i) => (
                  <li
                    key={`${String(i)}:${asString(w.name)}`}
                    className="flex items-center gap-2 text-sm"
                  >
                    <FolderOpen className="text-muted-foreground size-4" />
                    <span className="flex-1 truncate font-medium">{asString(w.name)}</span>
                    {w.state ? (
                      <span className="text-muted-foreground text-xs">{asString(w.state)}</span>
                    ) : null}
                    <Button variant="ghost" size="sm" onClick={onOpenDrive}>
                      Open
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>

        {/* Two-up summaries */}
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
          <Card className="py-4">
            <CardHeader className="pb-0">
              <CardTitle className="text-sm">Transfers</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-1.5">
              <SummaryRow icon={<ArrowUpFromLine />} label="Uploading" value={activeUploads} />
              <SummaryRow icon={<ArrowDownToLine />} label="Downloading" value={activeDownloads} />
              <SummaryRow
                icon={<AlertTriangle />}
                label="Failed"
                value={failedTransfers}
                danger={failedTransfers > 0}
              />
            </CardContent>
          </Card>

          <Card className="py-4">
            <CardHeader className="pb-0">
              <CardTitle className="text-sm">Cache &amp; offline</CardTitle>
            </CardHeader>
            <CardContent className="flex flex-col gap-1.5">
              <SummaryRow
                icon={<HardDrive />}
                label="Used"
                value={usedBytes !== null ? formatBytes(usedBytes) : 'unknown'}
              />
              <SummaryRow
                icon={<CheckCircle2 />}
                label="Available offline"
                value={pinnedBytes !== null ? formatBytes(pinnedBytes) : 'unknown'}
              />
              <SummaryRow
                icon={<AlertTriangle />}
                label="Waiting to sync"
                value={dirtyBytes !== null ? formatBytes(dirtyBytes) : 'unknown'}
                danger={dirtyCount > 0}
              />
            </CardContent>
          </Card>
        </div>

        {/* Safe-to-quit */}
        <div
          className={cn(
            'flex items-center gap-2 rounded-md border px-3 py-2 text-xs',
            safeToQuit
              ? 'border-border bg-muted/40 text-muted-foreground'
              : 'border-primary/30 bg-primary/10 text-primary',
          )}
        >
          {safeToQuit ? <ShieldCheck className="size-3.5" /> : <ShieldAlert className="size-3.5" />}
          <span>
            {safeToQuit
              ? 'Safe to quit — no unsynced work, no uploads in flight.'
              : 'Not safe to quit yet — unsynced work or uploads in flight.'}
          </span>
        </div>
      </div>
    </ScrollArea>
  );
}

function isActive(entry: Entry): boolean {
  const state = asString(entry.state);
  return (
    state === 'running' ||
    state === 'queued' ||
    state === 'pending' ||
    state === '' ||
    state === 'uploading' ||
    state === 'downloading'
  );
}

function isFailed(entry: Entry): boolean {
  const state = asString(entry.state);
  return state === 'failed' || state === 'error';
}

function ReadinessBanner({
  reachable,
  mounted,
  safeToEdit,
  mountPath,
  onOpenDrive,
}: {
  reachable: boolean;
  mounted: boolean;
  safeToEdit: boolean;
  mountPath: string;
  onOpenDrive: () => void;
}) {
  const tone = !reachable ? 'offline' : safeToEdit ? 'good' : 'warn';
  const headline = !reachable
    ? 'Offline'
    : !mounted
      ? 'Connected — no mount'
      : safeToEdit
        ? 'Ready to work'
        : 'Sync pending';
  const detail = !reachable
    ? 'Daemon unreachable. Showing last known state.'
    : !mounted
      ? 'No mounted drive is attached.'
      : safeToEdit
        ? 'Mounted and online. Safe to edit.'
        : 'Some work has not synced yet. Safe to read; hold off on destructive edits.';
  return (
    <Card className="py-4">
      <CardContent className="flex items-center gap-3">
        <span
          className={cn(
            'flex size-9 shrink-0 items-center justify-center rounded-full',
            tone === 'good' && 'bg-emerald-500/15 text-emerald-500',
            tone === 'warn' && 'bg-primary/15 text-primary',
            tone === 'offline' && 'bg-destructive/15 text-destructive',
          )}
        >
          {tone === 'good' ? (
            <ShieldCheck className="size-5" />
          ) : (
            <ShieldAlert className="size-5" />
          )}
        </span>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-semibold">{headline}</p>
          <p className="text-muted-foreground truncate text-xs" title={mountPath}>
            {detail}
            {mountPath ? ` · ${mountPath}` : ''}
          </p>
        </div>
        <Button size="sm" disabled={!mounted} onClick={onOpenDrive}>
          Open drive
        </Button>
      </CardContent>
    </Card>
  );
}

function AttentionCard({
  reachable,
  dirtyCount,
  dirtyBytes,
  conflictCount,
  lockCount,
  failedTransfers,
}: {
  reachable: boolean;
  dirtyCount: number;
  dirtyBytes: number | null;
  conflictCount: number;
  lockCount: number;
  failedTransfers: number;
}) {
  const items: string[] = [];
  if (!reachable) items.push('Daemon offline — changes will queue and sync on reconnect.');
  if (dirtyCount > 0)
    items.push(
      `${String(dirtyCount)} file(s) waiting to sync${
        dirtyBytes !== null ? ` (${formatBytes(dirtyBytes)})` : ''
      }.`,
    );
  if (conflictCount > 0) items.push(`${String(conflictCount)} conflict(s) need review.`);
  if (failedTransfers > 0) items.push(`${String(failedTransfers)} failed transfer(s).`);
  if (lockCount > 0) items.push(`${String(lockCount)} lock(s) held.`);
  if (items.length === 0) return null;
  return (
    <Card className="border-primary/30 py-4">
      <CardHeader className="pb-0">
        <CardTitle className="text-primary flex items-center gap-1.5 text-sm">
          <AlertTriangle className="size-3.5" />
          Attention required
        </CardTitle>
      </CardHeader>
      <CardContent>
        <ul className="flex flex-col gap-1">
          {items.map((line, i) => (
            <li key={String(i)} className="text-muted-foreground flex items-center gap-1.5 text-xs">
              <Lock className="size-3 opacity-0" aria-hidden="true" />
              {line}
            </li>
          ))}
        </ul>
      </CardContent>
    </Card>
  );
}

function SummaryRow({
  icon,
  label,
  value,
  danger,
}: {
  icon: React.ReactNode;
  label: string;
  value: number | string;
  danger?: boolean;
}) {
  return (
    <div className="flex items-center gap-2 text-sm">
      <span className={cn('text-muted-foreground', danger && 'text-primary')}>{icon}</span>
      <span className="text-muted-foreground flex-1">{label}</span>
      <span className={cn('font-medium tabular-nums', danger ? 'text-primary' : '')}>
        {typeof value === 'number' ? String(value) : value}
      </span>
    </div>
  );
}
