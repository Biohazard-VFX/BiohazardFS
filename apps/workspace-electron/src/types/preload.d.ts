export type DaemonStatusResult = {
  ok: boolean;
  endpoint: string;
  body?: unknown;
  error?: string;
};

export type CachePathParams = { path: string };
export type TransferIdParams = { transfer_id?: string };
export type ConfigSetParams = { key: string; value: string };
export type VersionInfo = { app: string; electron: string; chrome: string; node: string };

export type WindowChrome = 'auto' | 'native' | 'frameless';
export type Theme = 'light' | 'dark' | 'system';
export type Prefs = {
  windowChrome: WindowChrome;
  zoomFactor: number;
  theme: Theme;
  cacheLimitGB: number | null;
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
      cacheStatus: () => Promise<DaemonStatusResult>;
      cacheList: () => Promise<DaemonStatusResult>;
      cachePin: (params: CachePathParams) => Promise<DaemonStatusResult>;
      cacheDehydrate: (params: CachePathParams) => Promise<DaemonStatusResult>;
      transferList: () => Promise<DaemonStatusResult>;
      transferPause: (params: TransferIdParams) => Promise<DaemonStatusResult>;
      transferResume: (params: TransferIdParams) => Promise<DaemonStatusResult>;
      conflictList: () => Promise<DaemonStatusResult>;
      conflictPreserveAll: () => Promise<DaemonStatusResult>;
      lockList: () => Promise<DaemonStatusResult>;
      configSet: (params: ConfigSetParams) => Promise<DaemonStatusResult>;
      versions: () => Promise<VersionInfo>;
      rpc: (method: string, params?: Record<string, unknown>) => Promise<DaemonStatusResult>;
      openPath: (target: string) => Promise<{ ok: boolean; error: string | null }>;
      prefsGet: () => Promise<Prefs>;
      prefsSet: (patch: Partial<Prefs>) => Promise<Prefs>;
      appInfo: () => Promise<AppInfo>;
      minimizeWindow: () => void;
      toggleMaximize: () => void;
      closeWindow: () => void;
      onPrefsChanged: (cb: (prefs: Prefs) => void) => () => void;
    };
  }
}
