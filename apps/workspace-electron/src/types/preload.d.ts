export type DaemonStatusResult = {
  ok: boolean;
  endpoint: string;
  body?: unknown;
  error?: string;
};

export type CacheTargetParams = { path?: string; node_id?: string };
export type TransferIdParams = { transfer_id?: string };
export type LockIdParams = { lock_id: string };
export type LockExtendParams = { lock_id: string; extend_seconds: number };
export type ConfigSetParams = { key: string; value: string };
export type VersionInfo = { app: string; electron: string; chrome: string; node: string };

export type WindowChrome = 'auto' | 'native' | 'frameless';
export type Theme = 'light' | 'dark' | 'system';
export type ReleaseChannel = 'dev' | 'nightly' | 'alpha' | 'beta' | 'stable';
export type Prefs = {
  windowChrome: WindowChrome;
  zoomFactor: number;
  theme: Theme;
  cacheLimitGB: number | null;
  releaseChannel: ReleaseChannel;
  autoUpdateChecks: boolean;
};
export type UpdateStatus = {
  state: 'idle' | 'checking' | 'available' | 'not_available' | 'unavailable' | 'error';
  channel: ReleaseChannel;
  currentVersion: string;
  packaged: boolean;
  updateVersion?: string;
  message?: string;
  checkedAt?: string;
};
export type AppInfo = {
  platform: string;
  frameless: boolean;
  versions: VersionInfo;
};

declare global {
  interface Window {
    biohazardfs: {
      daemonStatus: () => Promise<DaemonStatusResult>;
      workspaceStatus: () => Promise<DaemonStatusResult>;
      workspaceList: (path?: string) => Promise<DaemonStatusResult>;
      workspaceMount: () => Promise<{
        ok: boolean;
        mountpoint: string | null;
        error: string | null;
      }>;
      cacheStatus: () => Promise<DaemonStatusResult>;
      cacheList: () => Promise<DaemonStatusResult>;
      cachePin: (params: CacheTargetParams) => Promise<DaemonStatusResult>;
      cacheDehydrate: (params: CacheTargetParams) => Promise<DaemonStatusResult>;
      transferList: () => Promise<DaemonStatusResult>;
      transferPause: (params: TransferIdParams) => Promise<DaemonStatusResult>;
      transferResume: (params: TransferIdParams) => Promise<DaemonStatusResult>;
      conflictList: () => Promise<DaemonStatusResult>;
      conflictPreserveAll: () => Promise<DaemonStatusResult>;
      lockList: () => Promise<DaemonStatusResult>;
      lockRelease: (params: LockIdParams) => Promise<DaemonStatusResult>;
      lockExtend: (params: LockExtendParams) => Promise<DaemonStatusResult>;
      configSet: (params: ConfigSetParams) => Promise<DaemonStatusResult>;
      versions: () => Promise<VersionInfo>;
      rpc: (method: string, params?: Record<string, unknown>) => Promise<DaemonStatusResult>;
      openPath: (target: string) => Promise<{ ok: boolean; error: string | null }>;
      prefsGet: () => Promise<Prefs>;
      prefsSet: (patch: Partial<Prefs>) => Promise<Prefs>;
      appInfo: () => Promise<AppInfo>;
      updateStatus: () => Promise<UpdateStatus>;
      updateCheck: () => Promise<UpdateStatus>;
      minimizeWindow: () => void;
      toggleMaximize: () => void;
      closeWindow: () => void;
      onPrefsChanged: (cb: (prefs: Prefs) => void) => () => void;
    };
  }
}
