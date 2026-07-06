import { useMemo, useState } from 'react';
import { ShieldAlert, ShieldCheck, Trash2 } from 'lucide-react';

import { useActions } from '@/app/root';
import { type DaemonSnapshot } from '@/lib/use-daemon';
import { isStubbed, METHOD_NOT_IMPLEMENTED } from '@/lib/daemon-capabilities';
import {
  type Entry,
  asNumber,
  asString,
  entryList,
  extractData,
  extractError,
  isDirtyEntry,
} from '@/lib/daemon';
import { formatBytes, formatCount } from '@/lib/format';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from '@/components/ui/dialog';
import { Progress } from '@/components/ui/progress';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { ViewError, ViewLoading, Feedback } from '@/components/view-states';

type Props = {
  snapshot: DaemonSnapshot;
  loaded: boolean;
};

export function CacheView({ snapshot, loaded }: Props) {
  const { dehydrateEntry } = useActions();
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  const status = extractData(snapshot.cacheStatus);
  const list = extractData(snapshot.cacheList);
  const statusError = extractError(snapshot.cacheStatus);
  const listError = extractError(snapshot.cacheList);

  const usedBytes = asNumber(status?.used_bytes ?? status?.bytes_used);
  const pinnedBytes = asNumber(status?.pinned_bytes);
  const dirtyBytes = asNumber(status?.dirty_bytes);
  const dirtyCount = asNumber(status?.dirty_count);
  const quotaBytes = asNumber(status?.quota_bytes ?? status?.total_bytes);
  const freeBytes = asNumber(
    status?.free_bytes ?? status?.disk_free_bytes ?? status?.available_bytes,
  );
  const LOW_DISK_BYTES = 5 * 1024 * 1024 * 1024;
  const lowDisk = freeBytes !== null && freeBytes < LOW_DISK_BYTES;

  const entries = entryList(list, ['entries', 'items']);
  const dirtyCountFromList = entries.filter((e) => isDirtyEntry(e)).length;
  const effectiveDirty = dirtyCount ?? dirtyCountFromList;

  const byDir = useMemo(() => groupByDirectory(entries), [entries]);

  if (!loaded && snapshot.cacheStatus === null && snapshot.cacheList === null) {
    return <ViewLoading rows={4} />;
  }
  if (statusError && !status) {
    return <ViewError label="Cache status unavailable" error={statusError} />;
  }

  const hasQuota = usedBytes !== null && quotaBytes !== null && quotaBytes > 0;
  const ratio = hasQuota ? Math.min(usedBytes / quotaBytes, 1) : 0;

  async function clearAll() {
    setConfirmOpen(false);
    // SAFETY INVARIANT (SPEC: clear-all-local-cache panic button): never evict
    // dirty data. If anything is unsynced, refuse with an explicit message
    // instead of partially clearing.
    if (effectiveDirty > 0) {
      setFeedback({
        ok: false,
        text: `${formatCount(effectiveDirty)} file(s) haven’t synced. Clear-all refused to protect them.`,
      });
      return;
    }
    setFeedback(null);
    setClearing(true);
    let failures = 0;
    for (const entry of entries) {
      const path = asString(entry.path);
      if (!path || isDirtyEntry(entry)) continue;
      const result = await dehydrateEntry(path);
      if (extractError(result)) failures += 1;
    }
    setClearing(false);
    setFeedback(
      failures === 0
        ? { ok: true, text: 'Local cache cleared.' }
        : {
            ok: false,
            text: `${String(failures)} entr${failures === 1 ? 'y' : 'ies'} could not be cleared.`,
          },
    );
  }

  return (
    <ScrollArea className="h-full">
      <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4">
        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">Cache usage</CardTitle>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            <div className="flex items-baseline gap-2">
              <span className="text-2xl font-semibold tracking-tight">
                {usedBytes !== null ? formatBytes(usedBytes) : 'unknown'}
              </span>
              {hasQuota ? (
                <span className="text-muted-foreground text-sm">of {formatBytes(quotaBytes)}</span>
              ) : null}
            </div>
            {hasQuota ? <Progress value={Math.round(ratio * 100)} className="h-1.5" /> : null}
            <div className="grid grid-cols-2 gap-3 pt-1">
              <Stat
                label="Available offline"
                value={pinnedBytes !== null ? formatBytes(pinnedBytes) : 'unknown'}
              />
              <Stat
                label="Waiting to sync"
                value={dirtyBytes !== null ? formatBytes(dirtyBytes) : 'unknown'}
                accent={effectiveDirty > 0}
              />
            </div>
          </CardContent>
        </Card>

        {effectiveDirty > 0 ? (
          <div className="border-primary/30 bg-primary/10 flex items-start gap-2 rounded-md border px-3 py-2 text-xs">
            <ShieldAlert className="text-primary mt-0.5 size-3.5 shrink-0" />
            <span>
              {`${formatCount(effectiveDirty)} file(s) have local changes that haven’t synced. They won’t be removed.`}
            </span>
          </div>
        ) : null}

        {feedback ? <Feedback ok={feedback.ok}>{feedback.text}</Feedback> : null}

        <div className="flex flex-wrap items-center gap-2">
          <Dialog open={confirmOpen} onOpenChange={setConfirmOpen}>
            <DialogTrigger asChild>
              <Button variant="destructive" disabled={clearing || entries.length === 0}>
                <Trash2 className="size-3.5" />
                {clearing ? 'Clearing…' : 'Clear all local cache'}
              </Button>
            </DialogTrigger>
            <DialogContent>
              <DialogHeader>
                <DialogTitle>Clear all local cache?</DialogTitle>
                <DialogDescription>
                  This removes local copies of synced files across the workspace. Cloud data is not
                  deleted. Files with unsynced local changes are always kept.
                </DialogDescription>
              </DialogHeader>
              {effectiveDirty > 0 ? (
                <p className="text-primary text-xs">
                  {`${formatCount(effectiveDirty)} unsynced file(s) will be kept and must be removed individually.`}
                </p>
              ) : null}
              <DialogFooter>
                <Button
                  variant="outline"
                  onClick={() => {
                    setConfirmOpen(false);
                  }}
                >
                  Cancel
                </Button>
                <Button variant="destructive" onClick={() => void clearAll()}>
                  Clear synced copies
                </Button>
              </DialogFooter>
            </DialogContent>
          </Dialog>

          <VerifyButton />
        </div>

        {lowDisk ? (
          <div className="border-primary/30 bg-primary/10 text-primary flex items-center gap-2 rounded-md border px-3 py-2 text-xs">
            <ShieldAlert className="size-3.5 shrink-0" />
            <span>
              Low disk space — {formatBytes(freeBytes)} free. Syncing may stall until space is
              freed.
            </span>
          </div>
        ) : null}

        <Separator />

        <Card className="py-4">
          <CardHeader className="pb-0">
            <CardTitle className="text-sm">By directory</CardTitle>
          </CardHeader>
          <CardContent>
            {byDir.length === 0 ? (
              <p className="text-muted-foreground text-xs">
                {listError
                  ? `Cache list unavailable (${listError.code}).`
                  : 'No cached files reported.'}
              </p>
            ) : (
              <ul className="flex flex-col gap-1.5">
                {byDir.map((row) => (
                  <li key={row.dir} className="flex items-center gap-2 text-sm" title={row.dir}>
                    <span className="text-muted-foreground min-w-0 flex-1 truncate">{row.dir}</span>
                    {row.dirty > 0 ? (
                      <span className="text-primary text-xs font-medium">
                        {`${String(row.dirty)} waiting`}
                      </span>
                    ) : null}
                    <span className="text-muted-foreground w-20 text-right font-mono text-xs">
                      {formatBytes(row.bytes)}
                    </span>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
      </div>
    </ScrollArea>
  );
}

function Stat({ label, value, accent }: { label: string; value: string; accent?: boolean }) {
  return (
    <div className="bg-muted/40 rounded-md border px-3 py-2">
      <p className="text-muted-foreground text-[0.65rem] tracking-widest uppercase">{label}</p>
      <p className={accent ? 'text-primary text-sm font-semibold' : 'text-sm font-semibold'}>
        {value}
      </p>
    </div>
  );
}

type DirRow = { dir: string; bytes: number; dirty: number };

function groupByDirectory(entries: Entry[]): DirRow[] {
  const map = new Map<string, DirRow>();
  for (const entry of entries) {
    const rawPath = asString(entry.path);
    if (!rawPath) continue;
    const dir = parentDir(rawPath);
    const row = map.get(dir) ?? { dir, bytes: 0, dirty: 0 };
    const size = asNumber(entry.size_bytes);
    if (size !== null) row.bytes += size;
    if (isDirtyEntry(entry)) row.dirty += 1;
    map.set(dir, row);
  }
  return [...map.values()].sort((a, b) => b.bytes - a.bytes);
}

function parentDir(path: string): string {
  const norm = path.replace(/\\/g, '/').replace(/\/+$/, '');
  const idx = norm.lastIndexOf('/');
  if (idx <= 0) return '/';
  return norm.slice(0, idx);
}

// cache.verify (spine) — integrity check. Reports a short outcome; the daemon's
// draft envelope fields are not relied upon beyond ok/error.
function VerifyButton() {
  const [busy, setBusy] = useState(false);
  const [feedback, setFeedback] = useState<{ ok: boolean; text: string } | null>(null);

  async function run() {
    if (isStubbed('cache.verify')) {
      setFeedback({ ok: false, text: 'Requires daemon support (cache.verify not built).' });
      return;
    }
    setFeedback(null);
    setBusy(true);
    const res = await window.biohazardfs.rpc('cache.verify', {});
    setBusy(false);
    const err = extractError(res);
    if (err?.code === METHOD_NOT_IMPLEMENTED) {
      setFeedback({ ok: false, text: 'Requires daemon support (cache.verify not built).' });
    } else if (err) {
      setFeedback({ ok: false, text: `Verify failed (${err.code}).` });
    } else {
      setFeedback({ ok: true, text: 'Cache verified.' });
    }
  }

  return (
    <span className="flex items-center gap-2">
      <Button variant="outline" size="sm" disabled={busy} onClick={() => void run()}>
        <ShieldCheck className="size-3.5" />
        {busy ? 'Verifying…' : 'Verify cache'}
      </Button>
      {feedback ? (
        <span className={feedback.ok ? 'text-muted-foreground text-xs' : 'text-primary text-xs'}>
          {feedback.text}
        </span>
      ) : null}
    </span>
  );
}
