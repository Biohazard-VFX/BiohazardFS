import { app, BrowserWindow, ipcMain } from 'electron';
import path from 'node:path';

const DAEMON_ENDPOINT = process.env.BIOHAZARDFS_DAEMON_ENDPOINT ?? '127.0.0.1:47666';
const LOCAL_TOKEN = process.env.BIOHAZARDFS_LOCAL_TOKEN ?? '';
const IS_SMOKE = process.env.BIOHAZARDFS_DESKTOP_SMOKE === '1';
const IS_DEV = process.env.NODE_ENV === 'development';

function isAllowedLoopbackEndpoint(endpoint: string): boolean {
  if (
    endpoint.includes('/') ||
    endpoint.includes('?') ||
    endpoint.includes('#') ||
    endpoint.includes('@')
  ) {
    return false;
  }

  try {
    const url = new URL(`http://${endpoint}`);
    const hostname = url.hostname.replace(/^\[|\]$/g, '');
    const hasPort = /^\d+$/.test(url.port);
    const isIpv4Loopback = /^127\.\d{1,3}\.\d{1,3}\.\d{1,3}$/.test(hostname);
    const isIpv6Loopback = hostname === '::1';
    return (
      hasPort &&
      (isIpv4Loopback || isIpv6Loopback) &&
      url.pathname === '/' &&
      !url.username &&
      !url.password
    );
  } catch {
    return false;
  }
}

async function daemonRpc(method: string, params: Record<string, unknown> = {}) {
  const endpoint = DAEMON_ENDPOINT;
  if (!isAllowedLoopbackEndpoint(endpoint)) {
    return {
      ok: false,
      endpoint,
      error: 'daemon endpoint must be an explicit loopback host and port',
    };
  }
  if (!LOCAL_TOKEN) {
    return { ok: false, endpoint, error: 'missing BIOHAZARDFS_LOCAL_TOKEN' };
  }

  try {
    const response = await fetch(`http://${endpoint}/rpc`, {
      method: 'POST',
      headers: {
        Accept: 'application/json',
        Authorization: `Bearer ${LOCAL_TOKEN}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        id: `req_ui_${String(Date.now())}`,
        method,
        params,
        meta: {
          source: 'ui',
          actor_hint: null,
          impersonated_user_id: null,
          schema_version: '2026-07-daemon-v1',
        },
      }),
    });
    const body = (await response.json()) as { ok?: boolean };
    return { ok: response.ok && body.ok === true, endpoint, body };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    return { ok: false, endpoint, error: message };
  }
}

async function daemonStatus() {
  return daemonRpc('daemon.status');
}

async function workspaceStatus() {
  return daemonRpc('workspace.status');
}

async function workspaceList(pathName = '') {
  return daemonRpc('workspace.list', { path: pathName });
}

function rendererEntry(): string {
  const devServerUrl = process.env.VITE_DEV_SERVER_URL;
  if (IS_DEV && devServerUrl?.startsWith('http://127.0.0.1:')) {
    return devServerUrl;
  }
  return `file://${path.join(__dirname, '../renderer/index.html')}`;
}

async function createWindow(): Promise<BrowserWindow> {
  const window = new BrowserWindow({
    width: 1040,
    height: 720,
    minWidth: 860,
    minHeight: 560,
    title: 'Biohazard Workspace',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  await window.loadURL(rendererEntry());
  return window;
}

ipcMain.handle('daemon:status', daemonStatus);
ipcMain.handle('workspace:status', workspaceStatus);
ipcMain.handle('workspace:list', (_event, pathName: string) => workspaceList(pathName));
ipcMain.handle('app:versions', () => ({
  app: app.getVersion(),
  electron: process.versions.electron,
  chrome: process.versions.chrome,
  node: process.versions.node,
}));

void app
  .whenReady()
  .then(async () => {
    const window = await createWindow();

    if (IS_SMOKE) {
      const status = await daemonStatus();
      const workspace = await workspaceStatus();
      console.log(
        JSON.stringify({
          ok: status.ok && workspace.ok,
          smoke: 'biohazard-workspace',
          daemon: status,
          workspace,
        }),
      );
      window.close();
      if (status.ok && workspace.ok) {
        app.quit();
      } else {
        app.exit(1);
      }
      return;
    }

    app.on('activate', () => {
      if (BrowserWindow.getAllWindows().length === 0) {
        void createWindow();
      }
    });
  })
  .catch((error: unknown) => {
    console.error(error);
    app.exit(1);
  });

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit();
  }
});
