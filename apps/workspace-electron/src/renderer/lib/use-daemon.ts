import { useCallback, useEffect, useRef, useState } from 'react';
import { type DaemonStatusResult, type VersionInfo, dirtyEntryCount, keepLastGood } from './daemon';

// Aggregated daemon state for the whole shell. This is the original renderer's
// fetchAll/applyResults pattern, lifted verbatim in behavior, plus:
//   - adaptive polling: ~3s while transfers are active or files are dirty,
//     ~15s when idle. Transfers and unsynced work are the time-critical
//     surfaces; idle reads cost less.
//   - paused while the window is hidden (no wasted daemon traffic).
//   - keepLastGood on every slice so a transient dropout never blanks the UI.
//
// All actions (pin/dehydrate/pause/resume/preserve) refresh only the affected
// slice via the daemon client, then re-merge with keepLastGood.

const POLL_ACTIVE_MS = 3000;
const POLL_IDLE_MS = 15000;

export type DaemonSnapshot = {
  daemon: DaemonStatusResult | null;
  workspace: DaemonStatusResult | null;
  workspaceList: DaemonStatusResult | null;
  cacheStatus: DaemonStatusResult | null;
  cacheList: DaemonStatusResult | null;
  transferList: DaemonStatusResult | null;
  conflictList: DaemonStatusResult | null;
  lockList: DaemonStatusResult | null;
  mountStatus: DaemonStatusResult | null;
  mountList: DaemonStatusResult | null;
  versions: VersionInfo | null;
};

async function fetchSnapshot(): Promise<DaemonSnapshot> {
  const [
    daemon,
    workspace,
    workspaceList,
    cacheStatus,
    cacheList,
    transferList,
    conflictList,
    lockList,
    mountStatus,
    mountList,
    versions,
  ] = await Promise.all([
    window.biohazardfs.daemonStatus(),
    window.biohazardfs.workspaceStatus(),
    window.biohazardfs.workspaceList(''),
    window.biohazardfs.cacheStatus(),
    window.biohazardfs.cacheList(),
    window.biohazardfs.transferList(),
    window.biohazardfs.conflictList(),
    window.biohazardfs.lockList(),
    window.biohazardfs.rpc('mount.status'),
    window.biohazardfs.rpc('mount.list'),
    window.biohazardfs.versions(),
  ]);
  return {
    daemon,
    workspace,
    workspaceList,
    cacheStatus,
    cacheList,
    transferList,
    conflictList,
    lockList,
    mountStatus,
    mountList,
    versions,
  };
}

function mergeSnapshot(prev: DaemonSnapshot, next: DaemonSnapshot): DaemonSnapshot {
  return {
    daemon: next.daemon, // endpoint + reachability always reflect latest
    workspace: keepLastGood(prev.workspace, next.workspace),
    workspaceList: keepLastGood(prev.workspaceList, next.workspaceList),
    cacheStatus: keepLastGood(prev.cacheStatus, next.cacheStatus),
    cacheList: keepLastGood(prev.cacheList, next.cacheList),
    transferList: keepLastGood(prev.transferList, next.transferList),
    conflictList: keepLastGood(prev.conflictList, next.conflictList),
    lockList: keepLastGood(prev.lockList, next.lockList),
    mountStatus: keepLastGood(prev.mountStatus, next.mountStatus),
    mountList: keepLastGood(prev.mountList, next.mountList),
    versions: next.versions ?? prev.versions,
  };
}

const EMPTY: DaemonSnapshot = {
  daemon: null,
  workspace: null,
  workspaceList: null,
  cacheStatus: null,
  cacheList: null,
  transferList: null,
  conflictList: null,
  lockList: null,
  mountStatus: null,
  mountList: null,
  versions: null,
};

export function useDaemonState() {
  const [snapshot, setSnapshot] = useState<DaemonSnapshot>(EMPTY);
  const [loaded, setLoaded] = useState(false);
  const [lastUpdated, setLastUpdated] = useState<number | null>(null);
  const inFlight = useRef(false);

  const apply = useCallback((next: DaemonSnapshot) => {
    setSnapshot((prev) => mergeSnapshot(prev, next));
    setLastUpdated(Date.now());
    setLoaded(true);
  }, []);

  const refresh = useCallback(async () => {
    if (inFlight.current) {
      return;
    }
    inFlight.current = true;
    try {
      apply(await fetchSnapshot());
    } finally {
      inFlight.current = false;
    }
  }, [apply]);

  // Initial load.
  useEffect(() => {
    let cancelled = false;
    void fetchSnapshot().then((snap) => {
      if (cancelled) return;
      apply(snap);
    });
    return () => {
      cancelled = true;
    };
  }, [apply]);

  // Adaptive poll: faster while transfers are running or files are dirty,
  // slower when idle. Paused while the document is hidden.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const tick = async () => {
      if (document.hidden) {
        schedule(POLL_IDLE_MS);
        return;
      }
      await refresh();
      schedule(activeInterval(snapshot));
    };

    const schedule = (ms: number) => {
      timer = setTimeout(() => {
        void tick();
      }, ms);
    };

    schedule(activeInterval(snapshot));
    return () => {
      if (timer) clearTimeout(timer);
    };
  }, [refresh, snapshot]);

  return { snapshot, loaded, lastUpdated, refresh };
}

function activeInterval(snap: DaemonSnapshot): number {
  // The daemon response envelope's `data` shape is draft, so we read defensively
  // and fall back to idle cadence when fields are missing.
  const transfers = snap.transferList?.body as
    { data?: { entries?: unknown[]; transfers?: unknown[]; items?: unknown[] } } | undefined;
  const transferCount =
    transfers?.data?.entries?.length ??
    transfers?.data?.transfers?.length ??
    transfers?.data?.items?.length ??
    0;
  const cacheStatus = snap.cacheStatus?.body as
    { data?: { dirty_count?: number; dirty_entries?: number; dirty_bytes?: number } } | undefined;
  const dirtyCount = dirtyEntryCount(cacheStatus?.data ?? null) ?? 0;
  const dirtyBytes = cacheStatus?.data?.dirty_bytes ?? 0;

  if (transferCount > 0 || dirtyCount > 0 || dirtyBytes > 0) {
    return POLL_ACTIVE_MS;
  }
  return POLL_IDLE_MS;
}
