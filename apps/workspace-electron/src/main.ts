import { app, BrowserWindow, ipcMain, Menu, nativeTheme, shell } from 'electron';
import path from 'node:path';

import { loadPrefs, resolveFrameless, savePrefs } from './main.prefs';
import { buildAppMenu } from './main.menu';

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

// Artist-facing surfaces. Each thin handler routes a generic daemon RPC and
// returns the daemon response envelope untouched. The renderer renders every
// response defensively; unknown/error envelopes are surfaced, never crashed on.

async function cacheStatus() {
  return daemonRpc('cache.status');
}

async function cacheList() {
  return daemonRpc('cache.list');
}

async function cachePin(params: { path: string }) {
  return daemonRpc('cache.pin', params);
}

async function cacheDehydrate(params: { path: string }) {
  return daemonRpc('cache.dehydrate', params);
}

async function transferList() {
  return daemonRpc('transfer.list');
}

async function transferPause(params: { transfer_id?: string }) {
  return daemonRpc('transfer.pause', params);
}

async function transferResume(params: { transfer_id?: string }) {
  return daemonRpc('transfer.resume', params);
}

async function conflictList() {
  return daemonRpc('conflict.list');
}

async function conflictPreserveAll() {
  return daemonRpc('conflict.preserve_all');
}

async function lockList() {
  return daemonRpc('lock.list');
}

async function configSet(params: { key: string; value: string }) {
  return daemonRpc('config.set', params);
}

function rendererEntry(): string {
  const devServerUrl = process.env.VITE_DEV_SERVER_URL;
  if (IS_DEV && devServerUrl?.startsWith('http://127.0.0.1:')) {
    return devServerUrl;
  }
  return `file://${path.join(__dirname, '../renderer/index.html')}`;
}

async function createWindow(): Promise<BrowserWindow> {
  const prefs = loadPrefs();
  const frameless = resolveFrameless(prefs, process.platform);
  const dark =
    prefs.theme === 'dark' || (prefs.theme === 'system' && nativeTheme.shouldUseDarkColors);
  const window = new BrowserWindow({
    width: 1200,
    height: 780,
    minWidth: 960,
    minHeight: 640,
    title: 'BiohazardFS',
    frame: !frameless,
    // Match the renderer's background so the window doesn't flash the wrong
    // color before React mounts. These mirror --background in globals.css:
    // dark oklch(0.2679 0.0036 106.6427) ≈ #262624; light oklch(0.9818 0.0054
    // 95.0986) ≈ #faf9f5.
    backgroundColor: dark ? '#262624' : '#faf9f5',
    webPreferences: {
      preload: path.join(__dirname, 'preload.js'),
      contextIsolation: true,
      nodeIntegration: false,
      sandbox: true,
    },
  });

  await window.loadURL(rendererEntry());
  // Restore persisted zoom after load so the renderer mounts at the user's
  // chosen scale. setZoomFactor after loadURL resolves is reliable.
  window.webContents.setZoomFactor(prefs.zoomFactor);
  return window;
}

ipcMain.handle('daemon:status', daemonStatus);
ipcMain.handle('workspace:status', workspaceStatus);
ipcMain.handle('workspace:list', (_event, pathName: string) => workspaceList(pathName));
ipcMain.handle('cache:status', cacheStatus);
ipcMain.handle('cache:list', cacheList);
ipcMain.handle('cache:pin', (_event, params: { path: string }) => cachePin(params));
ipcMain.handle('cache:dehydrate', (_event, params: { path: string }) => cacheDehydrate(params));
ipcMain.handle('transfer:list', transferList);
ipcMain.handle('transfer:pause', (_event, params: { transfer_id?: string }) =>
  transferPause(params),
);
ipcMain.handle('transfer:resume', (_event, params: { transfer_id?: string }) =>
  transferResume(params),
);
ipcMain.handle('conflict:list', conflictList);
ipcMain.handle('conflict:preserveAll', conflictPreserveAll);
ipcMain.handle('lock:list', lockList);
ipcMain.handle('config:set', (_event, params: { key: string; value: string }) => configSet(params));
ipcMain.handle('app:versions', () => ({
  app: app.getVersion(),
  electron: process.versions.electron,
  chrome: process.versions.chrome,
  node: process.versions.node,
}));

// Generic read-only RPC passthrough for views that need methods outside the
// always-polled global snapshot (workset.list, mount.status, audit.events,
// snapshot.list, etc.). Same daemonRpc + loopback/token safety; the renderer
// treats the envelope as untrusted draft data, same as everything else.
ipcMain.handle(
  'daemon:rpc',
  (_event, payload: { method: string; params?: Record<string, unknown> }) =>
    daemonRpc(payload.method, payload.params ?? {}),
);

// Open a folder in the OS file manager (Finder / Explorer / Files). Only ever
// invoked with the workspace root from the Connection view. shell.openPath
// returns '' on success or a localized error string.
ipcMain.handle('shell:openPath', async (_event, target: unknown) => {
  if (typeof target !== 'string' || target.length === 0) {
    return { ok: false, error: 'invalid path' };
  }
  try {
    const errorMessage = await shell.openPath(target);
    return { ok: errorMessage === '', error: errorMessage || null };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : String(error) };
  }
});

// --- UI prefs (Electron-owned presentation state, not daemon config) -------
ipcMain.handle('prefs:get', () => loadPrefs());

ipcMain.handle(
  'prefs:set',
  (
    _event,
    patch: Partial<{
      windowChrome: string;
      zoomFactor: number;
      theme: string;
      cacheLimitGB: number | null;
    }>,
  ) => {
    // Only accept known shapes; unknown keys are ignored. zoomFactor + cache
    // limit are clamped inside savePrefs so a bad value can't corrupt state.
    const safe: Partial<{
      windowChrome: 'auto' | 'native' | 'frameless';
      zoomFactor: number;
      theme: 'light' | 'dark' | 'system';
      cacheLimitGB: number | null;
    }> = {};
    if (
      patch.windowChrome === 'auto' ||
      patch.windowChrome === 'native' ||
      patch.windowChrome === 'frameless'
    ) {
      safe.windowChrome = patch.windowChrome;
    }
    if (typeof patch.zoomFactor === 'number') {
      safe.zoomFactor = patch.zoomFactor;
    }
    if (patch.theme === 'light' || patch.theme === 'dark' || patch.theme === 'system') {
      safe.theme = patch.theme;
    }
    if (patch.cacheLimitGB === null || typeof patch.cacheLimitGB === 'number') {
      safe.cacheLimitGB = patch.cacheLimitGB;
    }
    const next = savePrefs(safe);
    // Zoom applies live; window chrome needs a restart (frame is fixed at
    // creation), so only propagate zoom to live windows here.
    if (safe.zoomFactor !== undefined) {
      for (const win of BrowserWindow.getAllWindows()) {
        win.webContents.setZoomFactor(next.zoomFactor);
      }
    }
    broadcastPrefs(next);
    return next;
  },
);

function broadcastPrefs(prefs: { windowChrome: string; zoomFactor: number }) {
  for (const win of BrowserWindow.getAllWindows()) {
    win.webContents.send('prefs:changed', prefs);
  }
}

ipcMain.handle('app:info', () => {
  const prefs = loadPrefs();
  return {
    platform: process.platform,
    frameless: resolveFrameless(prefs, process.platform),
    versions: {
      app: app.getVersion(),
      electron: process.versions.electron,
      chrome: process.versions.chrome,
      node: process.versions.node,
    },
  };
});

// --- window controls for the frameless chrome ------------------------------
ipcMain.on('window:minimize', (event) => {
  BrowserWindow.fromWebContents(event.sender)?.minimize();
});
ipcMain.on('window:toggleMaximize', (event) => {
  const win = BrowserWindow.fromWebContents(event.sender);
  if (!win) return;
  if (win.isMaximized()) {
    win.unmaximize();
  } else {
    win.maximize();
  }
});
ipcMain.on('window:close', (event) => {
  BrowserWindow.fromWebContents(event.sender)?.close();
});

void app
  .whenReady()
  .then(async () => {
    Menu.setApplicationMenu(buildAppMenu());
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
