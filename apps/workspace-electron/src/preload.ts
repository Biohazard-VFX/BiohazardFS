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

type CachePathParams = { path: string };
type TransferIdParams = { transfer_id?: string };
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

async function cacheStatus(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:status')) as DaemonStatusResult;
}

async function cacheList(): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:list')) as DaemonStatusResult;
}

async function cachePin(params: CachePathParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('cache:pin', params)) as DaemonStatusResult;
}

async function cacheDehydrate(params: CachePathParams): Promise<DaemonStatusResult> {
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

async function configSet(params: ConfigSetParams): Promise<DaemonStatusResult> {
  return (await ipcRenderer.invoke('config:set', params)) as DaemonStatusResult;
}

async function versions(): Promise<VersionInfo> {
  return (await ipcRenderer.invoke('app:versions')) as VersionInfo;
}

contextBridge.exposeInMainWorld('biohazardfs', {
  daemonStatus,
  workspaceStatus,
  workspaceList,
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
  configSet,
  versions,
});
