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

async function daemonStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('daemon:status')) as DaemonStatusResult;
}

async function workspaceStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('workspace:status')) as DaemonStatusResult;
}

async function workspaceList(path = ''): Promise<WorkspaceListResult> {
  return (await ipcRenderer.invoke('workspace:list', path)) as WorkspaceListResult;
}

async function versions(): Promise<VersionInfo> {
  return (await ipcRenderer.invoke('app:versions')) as VersionInfo;
}

contextBridge.exposeInMainWorld('biohazardfs', {
  daemonStatus,
  workspaceStatus,
  workspaceList,
  versions,
});
