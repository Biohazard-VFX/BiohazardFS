import { contextBridge, ipcRenderer } from 'electron';

type DaemonStatusResult = {
  ok: boolean;
  endpoint: string;
  body?: unknown;
  error?: string;
};

type WorkspaceListResult = DaemonStatusResult;

type VersionInfo = {
  app: string;
  electron: string;
  chrome: string;
  node: string;
};

type CacheTargetParams = { path?: string; node_id?: string };
type TransferIdParams = { transfer_id?: string };
type LockIdParams = { lock_id: string };
type LockExtendParams = { lock_id: string; extend_seconds: number };
type ConfigSetParams = { key: string; value: string };

async function daemonStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('daemon:status')) as DaemonStatusResult;
}

async function workspaceStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('workspace:status')) as DaemonStatusResult;
}

async function workspaceList(path = ''): Promise<WorkspaceListResult> {
  return (await ipcRenderer.invoke('workspace:list', path)) as WorkspaceListResult;
}

async function workspaceMount(): Promise<{
  ok: boolean;
  mountpoint: string | null;
  error: string | null;
}> {
  return (await ipcRenderer.invoke('workspace:mount')) as {
    ok: boolean;
    mountpoint: string | null;
    error: string | null;
  };
}

async function cacheStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:status')) as DaemonStatusResult;
}

async function cacheList(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:list')) as DaemonStatusResult;
}

async function cachePin(params: CacheTargetParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:pin', params)) as DaemonStatusResult;
}

async function cacheDehydrate(params: CacheTargetParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:dehydrate', params)) as DaemonStatusResult;
}

async function transferList(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('transfer:list')) as DaemonStatusResult;
}

async function transferPause(params: TransferIdParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('transfer:pause', params)) as DaemonStatusResult;
}

async function transferResume(params: TransferIdParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('transfer:resume', params)) as DaemonStatusResult;
}

async function conflictList(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('conflict:list')) as DaemonStatusResult;
}

async function conflictPreserveAll(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('conflict:preserveAll')) as DaemonStatusResult;
}

async function lockList(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('lock:list')) as DaemonStatusResult;
}

async function lockRelease(params: LockIdParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('lock:release', params)) as DaemonStatusResult;
}

async function lockExtend(params: LockExtendParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('lock:extend', params)) as DaemonStatusResult;
}

async function configSet(params: ConfigSetParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('config:set', params)) as DaemonStatusResult;
}

type ServerSyncProfile = {
  serverUrl: string;
  serverToken: string;
  allowPlaintext: boolean;
};

type SyncResult = DaemonStatusResult & { profile?: ServerSyncProfile };

async function syncProfileGet(): Promise<ServerSyncProfile> {
  return (await ipcRenderer.invoke('sync:profile.get')) as ServerSyncProfile;
}

async function syncProfileSet(profile: ServerSyncProfile): Promise<SyncResult> {
  return (await ipcRenderer.invoke('sync:profile.set', profile)) as SyncResult;
}

async function syncStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('sync:status')) as DaemonStatusResult;
}

async function syncPush(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('sync:push')) as DaemonStatusResult;
}

async function syncPull(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('sync:pull')) as DaemonStatusResult;
}

async function versions(): Promise<VersionInfo> {
  return (await ipcRenderer.invoke('app:versions')) as VersionInfo;
}

// Generic read-only RPC for methods outside the always-polled global snapshot
// (workset.list, mount.status, audit.events, …). The main process enforces an
// allowlist; mutating actions use dedicated helpers above.
async function rpc(
  method: string,
  params: Record<string, unknown> = {},
): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('daemon:rpc', { method, params })) as DaemonStatusResult;
}

// Open a folder in the OS file manager. Returns {ok, error}.
async function openPath(target: string): Promise<{ ok: boolean; error: string | null }> {
  return (await ipcRenderer.invoke('shell:openPath', target)) as {
    ok: boolean;
    error: string | null;
  };
}

// UI prefs + window chrome. These are presentation concerns owned by Electron,
// not the daemon. prefs.set persists to userData/prefs.json in the main process.
type WindowChrome = 'auto' | 'native' | 'frameless';
type Theme = 'light' | 'dark' | 'system';
type ReleaseChannel = 'dev' | 'nightly' | 'alpha' | 'beta' | 'stable';
type Prefs = {
  windowChrome: WindowChrome;
  zoomFactor: number;
  theme: Theme;
  cacheLimitGB: number | null;
  releaseChannel: ReleaseChannel;
  autoUpdateChecks: boolean;
};
type UpdateStatus = {
  state: 'idle' | 'checking' | 'available' | 'not_available' | 'unavailable' | 'error';
  channel: ReleaseChannel;
  currentVersion: string;
  packaged: boolean;
  updateVersion?: string;
  message?: string;
  checkedAt?: string;
};
type AppInfo = {
  platform: string;
  frameless: boolean;
  versions: VersionInfo;
};

async function prefsGet(): Promise<Prefs> {
  return (await ipcRenderer.invoke('prefs:get')) as Prefs;
}

async function prefsSet(patch: Partial<Prefs>): Promise<Prefs> {
  return (await ipcRenderer.invoke('prefs:set', patch)) as Prefs;
}

async function appInfo(): Promise<AppInfo> {
  return (await ipcRenderer.invoke('app:info')) as AppInfo;
}

async function updateStatus(): Promise<UpdateStatus> {
  return (await ipcRenderer.invoke('updates:status')) as UpdateStatus;
}

async function updateCheck(): Promise<UpdateStatus> {
  return (await ipcRenderer.invoke('updates:check')) as UpdateStatus;
}

// Subscribe to prefs changes pushed from main (so the Settings UI stays in sync
// when zoom is changed via the Ctrl+± keyboard shortcuts). Returns an unsub.
function onPrefsChanged(cb: (prefs: Prefs) => void): () => void {
  const listener = (_event: unknown, prefs: Prefs) => {
    cb(prefs);
  };
  ipcRenderer.on('prefs:changed', listener);
  return () => {
    ipcRenderer.removeListener('prefs:changed', listener);
  };
}

function minimizeWindow(): void {
  ipcRenderer.send('window:minimize');
}

function toggleMaximize(): void {
  ipcRenderer.send('window:toggleMaximize');
}

function closeWindow(): void {
  ipcRenderer.send('window:close');
}

contextBridge.exposeInMainWorld('biohazardfs', {
  daemonStatus,
  workspaceStatus,
  workspaceList,
  workspaceMount,
  cacheStatus,
  cacheList,
  cachePin,
  cacheDehydrate,
  transferList,
  transferPause,
  transferResume,
  conflictList,
  conflictPreserveAll,
  lockList,
  lockRelease,
  lockExtend,
  configSet,
  syncProfileGet,
  syncProfileSet,
  syncStatus,
  syncPush,
  syncPull,
  versions,
  rpc,
  openPath,
  prefsGet,
  prefsSet,
  appInfo,
  updateStatus,
  updateCheck,
  minimizeWindow,
  toggleMaximize,
  closeWindow,
  onPrefsChanged,
});
