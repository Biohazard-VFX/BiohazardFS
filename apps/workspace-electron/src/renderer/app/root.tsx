import { createContext, useCallback, useContext, useMemo, useState } from 'react';

import { type FilesViewMode, type ViewId } from '@/app/nav';
import { AppSidebar } from '@/components/app-sidebar';
import { StudioRail } from '@/components/studio-rail';
import { Topbar } from '@/components/topbar';
import { StatusBar } from '@/components/status-bar';
import { FilesView } from '@/components/views/files-view';
import { ActivityView } from '@/components/views/activity-view';
import { CacheView } from '@/components/views/cache-view';
import { ConflictsView } from '@/components/views/conflicts-view';
import { Onboarding } from '@/components/onboarding/onboarding';
import { MyWorkView } from '@/components/views/my-work-view';
import { ConnectionView } from '@/components/views/connection-view';
import { LocksView } from '@/components/views/locks-view';
import { AuditView } from '@/components/views/audit-view';
import { SnapshotsView } from '@/components/views/snapshots-view';
import { AccessView } from '@/components/views/access-view';
import { PlaceholderView } from '@/components/views/placeholder-view';
import { SettingsView } from '@/components/views/settings-view';
import { type DaemonStatusResult, extractData } from '@/lib/daemon';
import { daemonReachable, deriveCounts } from '@/lib/derive';
import { useDaemonState } from '@/lib/use-daemon';
import { useAppInfo, useTheme } from '@/lib/use-prefs';

// The shell. Holds view-state and the query string for the topbar filter, and
// composes the sidebar, topbar, active view, and status bar. All daemon state
// lives in useDaemonState; actions perform a daemon call then trigger a full
// refresh. A full refresh after each action is deliberate: the daemon is local
// loopback, the fetch is cheap, and a single source of truth beats per-slice
// patching that can drift out of sync. keepLastGood in the hook means a
// refresh during a transient dropout never blanks the panels.

export type Actions = {
  pinEntry: (path: string) => Promise<DaemonStatusResult>;
  dehydrateEntry: (path: string) => Promise<DaemonStatusResult>;
  pauseTransfers: () => Promise<DaemonStatusResult>;
  resumeTransfers: () => Promise<DaemonStatusResult>;
  preserveAllConflicts: () => Promise<DaemonStatusResult>;
  saveCacheLocation: (path: string) => Promise<DaemonStatusResult>;
};

const ActionContext = createContext<Actions | null>(null);

export function useActions(): Actions {
  const ctx = useContext(ActionContext);
  if (!ctx) {
    throw new Error('useActions must be used inside <Root />');
  }
  return ctx;
}

export function Root() {
  const { snapshot, loaded, lastUpdated, refresh } = useDaemonState();
  const appInfo = useAppInfo();
  useTheme();
  const [view, setView] = useState<ViewId>('my-work');
  const [query, setQuery] = useState('');
  // First-run onboarding: shows on a genuine unconfigured workspace, and can be
  // opened manually from the studio rail for review (the seeded daemon always
  // has a workspace, so it won't auto-trigger in the preview).
  const [onboardingOpen, setOnboardingOpen] = useState(false);
  // Files layout preference (list vs tree). Persisted to localStorage so it
  // survives relaunches; it's a renderer-only view pref, not daemon state.
  const [filesMode, setFilesModeState] = useState<FilesViewMode>(() => {
    try {
      return localStorage.getItem('biohazardfs.filesView') === 'tree' ? 'tree' : 'list';
    } catch {
      return 'list';
    }
  });
  const setFilesMode = useCallback((mode: FilesViewMode) => {
    setFilesModeState(mode);
    try {
      localStorage.setItem('biohazardfs.filesView', mode);
    } catch {
      // ignore
    }
  }, []);

  const counts = useMemo(() => deriveCounts(snapshot), [snapshot]);
  const reachable = loaded && daemonReachable(snapshot);

  const workspaceData = extractData(snapshot.workspace);
  const workspaceReady = workspaceData?.state === 'ready';

  const actions = useMemo<Actions>(
    () => ({
      async pinEntry(path: string) {
        const result = await window.biohazardfs.cachePin({ path });
        await refresh();
        return result;
      },
      async dehydrateEntry(path: string) {
        const result = await window.biohazardfs.cacheDehydrate({ path });
        await refresh();
        return result;
      },
      async pauseTransfers() {
        const result = await window.biohazardfs.transferPause({});
        await refresh();
        return result;
      },
      async resumeTransfers() {
        const result = await window.biohazardfs.transferResume({});
        await refresh();
        return result;
      },
      async preserveAllConflicts() {
        const result = await window.biohazardfs.conflictPreserveAll();
        await refresh();
        return result;
      },
      async saveCacheLocation(path: string) {
        const result = await window.biohazardfs.configSet({
          key: 'cache.path',
          value: path,
        });
        await refresh();
        return result;
      },
    }),
    [refresh],
  );

  // Drive the refresh icon while an action is in flight. The hook guards
  // against concurrent fetches, so this is purely visual.
  const [refreshing, setRefreshing] = useState(false);
  // Bumped on each manual refresh so views with their own path state (Files)
  // know to re-fetch their current directory, not just the workspace root.
  const [refreshNonce, setRefreshNonce] = useState(0);
  const handleRefresh = useCallback(() => {
    setRefreshing(true);
    setRefreshNonce((n) => n + 1);
    void refresh().finally(() => {
      setRefreshing(false);
    });
  }, [refresh]);

  const realFirstRun = loaded && daemonReachable(snapshot) && !workspaceReady;
  if (realFirstRun || onboardingOpen) {
    return (
      <Onboarding
        snapshot={snapshot}
        onClose={() => {
          setOnboardingOpen(false);
        }}
        onOpenDrive={() => {
          setOnboardingOpen(false);
          setView('drive');
        }}
      />
    );
  }

  return (
    <ActionContext.Provider value={actions}>
      <div className="text-foreground flex h-screen w-screen overflow-hidden bg-background">
        <StudioRail
          reachable={daemonReachable(snapshot)}
          onShowOnboarding={() => {
            setOnboardingOpen(true);
          }}
        />
        <AppSidebar
          view={view}
          onViewChange={setView}
          counts={counts}
          studioLabel="Local workspace"
          workspaceReady={workspaceReady}
          reachable={daemonReachable(snapshot)}
        />

        <div className="flex min-w-0 flex-1 flex-col">
          <Topbar
            view={view}
            query={query}
            onQueryChange={setQuery}
            refreshing={refreshing}
            onRefresh={handleRefresh}
            transferCount={counts.transferCount}
            dirtyCount={counts.dirtyCount}
            reachable={reachable}
            frameless={appInfo?.frameless ?? false}
            filesMode={filesMode}
            onFilesModeChange={setFilesMode}
          />

          {loaded && !daemonReachable(snapshot) ? <OfflineBanner /> : null}

          <main className="min-h-0 flex-1 overflow-hidden">
            {view === 'my-work' ? (
              <MyWorkView
                snapshot={snapshot}
                loaded={loaded}
                onOpenDrive={() => {
                  setView('drive');
                }}
              />
            ) : view === 'connection' ? (
              <ConnectionView snapshot={snapshot} refreshNonce={refreshNonce} />
            ) : view === 'drive' ? (
              <FilesView
                query={query}
                snapshot={snapshot}
                loaded={loaded}
                refreshNonce={refreshNonce}
                mode={filesMode}
              />
            ) : view === 'transfers' ? (
              <ActivityView snapshot={snapshot} loaded={loaded} />
            ) : view === 'cache' ? (
              <CacheView snapshot={snapshot} loaded={loaded} />
            ) : view === 'conflicts' ? (
              <ConflictsView snapshot={snapshot} loaded={loaded} query={query} />
            ) : view === 'settings' ? (
              <SettingsView snapshot={snapshot} />
            ) : view === 'locks' ? (
              <LocksView refreshNonce={refreshNonce} />
            ) : view === 'audit' ? (
              <AuditView refreshNonce={refreshNonce} />
            ) : view === 'snapshots' ? (
              <SnapshotsView refreshNonce={refreshNonce} />
            ) : view === 'access' ? (
              <AccessView refreshNonce={refreshNonce} />
            ) : (
              <PlaceholderView
                view={view}
                note="Reserved. This admin surface requires daemon support (admin.* methods return method_not_implemented)."
              />
            )}
          </main>

          <StatusBar
            endpoint={snapshot.daemon?.endpoint ?? null}
            reachable={daemonReachable(snapshot)}
            appVersion={snapshot.versions?.app ?? null}
            lastUpdated={lastUpdated}
          />
        </div>
      </div>
    </ActionContext.Provider>
  );
}

function OfflineBanner() {
  return (
    <div
      role="status"
      className="border-destructive/30 bg-destructive/10 text-foreground flex items-center gap-2 px-4 py-1.5 text-xs"
    >
      <span className="font-medium">Daemon offline</span>
      <span className="text-muted-foreground">— showing last known state.</span>
    </div>
  );
}
