export type DaemonStatusResult = {
  ok: boolean;
  endpoint: string;
  body?: unknown;
  error?: string;
};

declare global {
  interface Window {
    biohazardfs: {
      daemonStatus: () => Promise<DaemonStatusResult>;
      workspaceStatus: () => Promise<DaemonStatusResult>;
      workspaceList: (path?: string) => Promise<DaemonStatusResult>;
      versions: () => Promise<{ app: string; electron: string; chrome: string; node: string }>;
    };
  }
}
