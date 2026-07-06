import { type DaemonSnapshot } from './use-daemon';
import { type Entry, asNumber, entryList, extractData, extractError, hasBody } from './daemon';

// Derived counts/status surfaced in the sidebar + sync pill. Reads are
// defensive: the daemon's draft envelopes may omit any field, so every accessor
// falls back to a safe default (0 / false) rather than throwing.

export type DaemonCounts = {
  transferCount: number;
  dirtyCount: number;
  conflictCount: number;
  lockCount: number;
  usedBytes: number | null;
  pinnedBytes: number | null;
  dirtyBytes: number | null;
  quotaBytes: number | null;
};

export function deriveCounts(snap: DaemonSnapshot): DaemonCounts {
  const cacheStatusData = extractData(snap.cacheStatus);
  const cacheListData = extractData(snap.cacheList);
  const transferData = extractData(snap.transferList);
  const conflictData = extractData(snap.conflictList);
  const lockData = extractData(snap.lockList);

  // Dirty total comes from cache.status if present, else counts dirty entries
  // in cache.list. Prefer the explicit daemon count to avoid undercounting.
  const statusDirty = asNumber(cacheStatusData?.dirty_count);
  const listDirty = entryList(cacheListData, ['entries', 'items']).filter(
    (e) => e.dirty === true,
  ).length;

  return {
    transferCount: entryList(transferData, ['entries', 'transfers', 'items']).filter(
      isActiveTransfer,
    ).length,
    dirtyCount: statusDirty ?? listDirty,
    conflictCount: entryList(conflictData, ['entries', 'conflicts', 'items']).length,
    lockCount: entryList(lockData, ['entries', 'locks', 'items']).length,
    usedBytes: asNumber(cacheStatusData?.used_bytes ?? cacheStatusData?.bytes_used),
    pinnedBytes: asNumber(cacheStatusData?.pinned_bytes),
    dirtyBytes: asNumber(cacheStatusData?.dirty_bytes),
    quotaBytes: asNumber(cacheStatusData?.quota_bytes ?? cacheStatusData?.total_bytes),
  };
}

function isActiveTransfer(entry: Entry): boolean {
  const state = typeof entry.state === 'string' ? entry.state : '';
  return (
    state === 'running' ||
    state === 'queued' ||
    state === 'pending' ||
    state === '' ||
    state === 'uploading' ||
    state === 'downloading'
  );
}

export function daemonReachable(snap: DaemonSnapshot): boolean {
  // Reachability is the only signal we read from the raw envelope: a missing
  // body means the main-process fetch never reached the daemon.
  return hasBody(snap.daemon);
}

export function daemonError(snap: DaemonSnapshot): { code: string; message: string } | null {
  return extractError(snap.daemon);
}
