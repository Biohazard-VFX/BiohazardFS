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
    };
  }
}
