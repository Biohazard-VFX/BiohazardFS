import { app, BrowserWindow, ipcMain, Menu, nativeTheme, shell } from 'electron';
import { spawn, spawnSync, type ChildProcess } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fsSync from 'node:fs';
import fs from 'node:fs/promises';
import path from 'node:path';

import { loadPrefs, resolveFrameless, savePrefs, type ReleaseChannel } from './main.prefs';
import { buildAppMenu } from './main.menu';
import { checkForUpdates, configureAutoUpdater, getUpdateStatus } from './main.updates';

const DAEMON_ENDPOINT = process.env.BIOHAZARDFS_DAEMON_ENDPOINT ?? '127.0.0.1:47666';
let localToken = process.env.BIOHAZARDFS_LOCAL_TOKEN ?? '';
let managedDaemon: ChildProcess | null = null;
let managedWorkspaceMount: { child: ChildProcess; mountpoint: string; cacheDir: string } | null =
  null;
let mountWorkspaceInFlight: Promise<{
  ok: boolean;
  mountpoint: string | null;
  error: string | null;
}> | null = null;
const IS_SMOKE = process.env.BIOHAZARDFS_DESKTOP_SMOKE === '1';
const IS_DEV = process.env.NODE_ENV === 'development';
const DISABLE_DAEMON_AUTOSTART = process.env.BIOHAZARDFS_DISABLE_DAEMON_AUTOSTART === '1';
const DEFAULT_MAC_VOLUME_MOUNTPOINT = '/Volumes/Biohazard';

const GENERIC_RENDERER_RPC_ALLOWLIST = new Set([
  'auth.status',
  'auth.whoami',
  'mount.status',
  'mount.list',
  'workset.list',
  'lock.list',
  'cache.verify',
  'audit.events',
  'snapshot.list',
  'grant.list',
  'share.list',
  'invite.list',
]);

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

async function fileExists(filePath: string): Promise<boolean> {
  try {
    await fs.access(filePath);
    return true;
  } catch {
    return false;
  }
}

function packagedBinDir(): string {
  const platform =
    process.platform === 'win32' ? 'win' : process.platform === 'darwin' ? 'mac' : 'linux';
  const arch = process.arch === 'x64' ? 'x64' : process.arch === 'arm64' ? 'arm64' : process.arch;
  return `${platform}-${arch}`;
}

async function bundledBinaryPath(
  binaryBaseName: string,
  overrideEnv: string,
): Promise<string | null> {
  const binaryName = process.platform === 'win32' ? `${binaryBaseName}.exe` : binaryBaseName;
  const override = process.env[overrideEnv];
  const candidates = [
    override,
    app.isPackaged
      ? path.join(process.resourcesPath, 'bin', packagedBinDir(), binaryName)
      : undefined,
    path.resolve(__dirname, '../../../../target/debug', binaryName),
    path.resolve(__dirname, '../../../../target/release', binaryName),
  ].filter(Boolean) as string[];

  for (const candidate of candidates) {
    if (await fileExists(candidate)) return candidate;
  }
  return null;
}

async function daemonBinaryPath(): Promise<string | null> {
  return bundledBinaryPath('biohazardfsd', 'BIOHAZARDFS_DAEMON_BIN');
}

async function fuseBinaryPath(): Promise<string | null> {
  return bundledBinaryPath('biohazardfs-fuse', 'BIOHAZARDFS_FUSE_BIN');
}

async function ensureLocalToken(): Promise<string> {
  if (localToken) return localToken;

  const tokenPath = path.join(app.getPath('userData'), 'daemon-token');
  try {
    const existing = (await fs.readFile(tokenPath, 'utf8')).trim();
    if (existing) {
      localToken = existing;
      return localToken;
    }
  } catch {
    // Missing token file is expected on first launch.
  }

  localToken = `bfsd_${randomBytes(32).toString('base64url')}`;
  await fs.mkdir(path.dirname(tokenPath), { recursive: true });
  await fs.writeFile(tokenPath, `${localToken}\n`, { mode: 0o600 });
  await fs.chmod(tokenPath, 0o600).catch(() => undefined);
  return localToken;
}

async function waitForDaemonReady(timeoutMs = 2500): Promise<void> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    const status = await daemonStatus();
    if (status.ok) return;
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
}

function defaultWorkspaceRoot(): string {
  if (process.env.BIOHAZARDFS_WORKSPACE_ROOT) return process.env.BIOHAZARDFS_WORKSPACE_ROOT;
  if (process.platform === 'darwin') return DEFAULT_MAC_VOLUME_MOUNTPOINT;
  return path.join(app.getPath('home'), 'Biohazard');
}

async function ensureManagedDaemon(): Promise<void> {
  if (IS_SMOKE || DISABLE_DAEMON_AUTOSTART || managedDaemon) return;
  if (!isAllowedLoopbackEndpoint(DAEMON_ENDPOINT)) return;

  await ensureLocalToken();
  const current = await daemonStatus();
  if (current.ok) return;

  const binary = await daemonBinaryPath();
  if (!binary) return;

  const child = spawn(binary, ['--dev-loopback-http', '--addr', DAEMON_ENDPOINT], {
    env: {
      ...process.env,
      BIOHAZARDFS_LOCAL_TOKEN: localToken,
      BIOHAZARDFS_WORKSPACE_ROOT: defaultWorkspaceRoot(),
    },
    stdio: ['ignore', 'ignore', 'pipe'],
    windowsHide: true,
  });
  managedDaemon = child;

  child.stderr.on('data', (chunk: Buffer) => {
    const message = chunk.toString('utf8').trim();
    if (message) console.warn(`[biohazardfsd] ${message}`);
  });
  child.on('exit', () => {
    if (managedDaemon === child) managedDaemon = null;
  });

  await waitForDaemonReady();
}

async function ensureDirectory(dir: string, mode = 0o755): Promise<void> {
  await fs.mkdir(dir, { recursive: true, mode });
  await fs.chmod(dir, mode).catch(() => undefined);
}

async function mountLineForPath(mountpoint: string): Promise<string | null> {
  if (process.platform === 'win32') return null;
  const { execFile } = await import('node:child_process');
  try {
    return await new Promise<string | null>((resolve) => {
      execFile('/sbin/mount', [], (error, stdout) => {
        if (error) {
          resolve(null);
          return;
        }
        resolve(stdout.split('\n').find((line) => line.includes(` on ${mountpoint} `)) ?? null);
      });
    });
  } catch {
    return null;
  }
}

async function isMountedPath(mountpoint: string): Promise<boolean> {
  if (process.platform === 'linux') {
    const { execFile } = await import('node:child_process');
    return await new Promise<boolean>((resolve) => {
      execFile('mountpoint', ['-q', mountpoint], (error) => {
        resolve(!error);
      });
    });
  }
  return (await mountLineForPath(mountpoint)) !== null;
}

async function isBiohazardMount(mountpoint: string): Promise<boolean> {
  const line = await mountLineForPath(mountpoint);
  if (!line) return false;
  if (process.platform === 'darwin') {
    return line.startsWith('Biohazard on ') && line.includes('macfuse');
  }
  return line.includes('fuse.biohazardfs') || line.includes('biohazardfs-fuse');
}

async function directoryIsEmpty(dir: string): Promise<boolean> {
  const entries = await fs.readdir(dir);
  return entries.length === 0;
}

async function ensureMacVolumeMountpoint(): Promise<string | null> {
  if (process.platform !== 'darwin') return null;
  if (await fileExists(DEFAULT_MAC_VOLUME_MOUNTPOINT)) return DEFAULT_MAC_VOLUME_MOUNTPOINT;
  const uid = typeof process.getuid === 'function' ? process.getuid() : 0;
  const gid = typeof process.getgid === 'function' ? process.getgid() : 0;
  const script = `do shell script "mkdir -p ${DEFAULT_MAC_VOLUME_MOUNTPOINT}; chown ${uid}:${gid} ${DEFAULT_MAC_VOLUME_MOUNTPOINT} || true; chmod 755 ${DEFAULT_MAC_VOLUME_MOUNTPOINT} || true" with administrator privileges`;
  spawnSync('/usr/bin/osascript', ['-e', script], { stdio: 'ignore' });
  return (await fileExists(DEFAULT_MAC_VOLUME_MOUNTPOINT)) ? DEFAULT_MAC_VOLUME_MOUNTPOINT : null;
}

function applyMacVolumeIcon(mountpoint: string): void {
  if (process.platform !== 'darwin') return;
  const iconPath = path.join(process.resourcesPath, 'icon.icns');
  try {
    // Do not use cp/fcopyfile here: macFUSE does not implement the fcopyfile
    // fast path, which can leave a zero-filled partial .VolumeIcon.icns while
    // still setting Finder's custom-icon bit.
    const icon = fsSync.readFileSync(iconPath);
    fsSync.writeFileSync(path.join(mountpoint, '.VolumeIcon.icns'), icon, { mode: 0o644 });
    spawnSync('/usr/bin/SetFile', ['-a', 'C', mountpoint], { stdio: 'ignore', timeout: 10000 });
  } catch (error) {
    console.warn(
      `[biohazardfs-fuse] could not apply Finder volume icon: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
  }
}

async function waitForMountReady(mountpoint: string, timeoutMs = 4000): Promise<boolean> {
  const startedAt = Date.now();
  while (Date.now() - startedAt < timeoutMs) {
    if (await isMountedPath(mountpoint)) return true;
    if (managedWorkspaceMount && managedWorkspaceMount.mountpoint === mountpoint) {
      if (managedWorkspaceMount.child.exitCode !== null) return false;
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  return false;
}

async function mountWorkspace(): Promise<{
  ok: boolean;
  mountpoint: string | null;
  error: string | null;
}> {
  if (mountWorkspaceInFlight) return mountWorkspaceInFlight;
  mountWorkspaceInFlight = mountWorkspaceInner()
    .catch((error: unknown) => ({
      ok: false,
      mountpoint: null,
      error: error instanceof Error ? error.message : String(error),
    }))
    .finally(() => {
      mountWorkspaceInFlight = null;
    });
  return mountWorkspaceInFlight;
}

async function mountWorkspaceInner(): Promise<{
  ok: boolean;
  mountpoint: string | null;
  error: string | null;
}> {
  if (managedWorkspaceMount && (await isMountedPath(managedWorkspaceMount.mountpoint))) {
    return { ok: true, mountpoint: managedWorkspaceMount.mountpoint, error: null };
  }
  if (process.platform === 'win32') {
    return {
      ok: false,
      mountpoint: null,
      error: 'workspace FUSE mount is not available on Windows yet',
    };
  }

  await ensureManagedDaemon();
  const token = await ensureLocalToken();
  const fuse = await fuseBinaryPath();
  if (!fuse) {
    return { ok: false, mountpoint: null, error: 'biohazardfs-fuse binary was not found' };
  }

  const volumeMountpoint = await ensureMacVolumeMountpoint();
  if (process.platform === 'darwin' && !volumeMountpoint) {
    return {
      ok: false,
      mountpoint: null,
      error:
        'Could not create /Volumes/Biohazard. macOS administrator approval is required for the default Finder-visible mount.',
    };
  }
  const mountpoint = volumeMountpoint ?? path.join(app.getPath('home'), 'Biohazard');
  const cacheDir = path.join(app.getPath('userData'), 'cache', 'workspace');
  await ensureDirectory(mountpoint, 0o755);
  await ensureDirectory(cacheDir, 0o700);
  if (await isMountedPath(mountpoint)) {
    if (await isBiohazardMount(mountpoint)) {
      return { ok: true, mountpoint, error: null };
    }
    return {
      ok: false,
      mountpoint: null,
      error: `${mountpoint} is already occupied by another mounted volume`,
    };
  }
  if (!(await directoryIsEmpty(mountpoint))) {
    return {
      ok: false,
      mountpoint: null,
      error: `${mountpoint} already exists and is not empty; move its contents or choose another mount path before mounting`,
    };
  }

  const child = spawn(
    fuse,
    [
      'mount-workspace',
      '--daemon-endpoint',
      DAEMON_ENDPOINT,
      '--cache-dir',
      cacheDir,
      '--mountpoint',
      mountpoint,
    ],
    {
      env: { ...process.env, BIOHAZARDFS_LOCAL_TOKEN: token },
      stdio: ['ignore', 'ignore', 'pipe'],
      windowsHide: true,
    },
  );
  managedWorkspaceMount = { child, mountpoint, cacheDir };
  let stderr = '';
  child.stderr.on('data', (chunk: Buffer) => {
    const message = chunk.toString('utf8');
    stderr += message;
    if (message.trim()) console.warn(`[biohazardfs-fuse] ${message.trim()}`);
  });
  child.on('error', (error) => {
    stderr += error instanceof Error ? error.message : String(error);
    if (managedWorkspaceMount?.child === child) managedWorkspaceMount = null;
  });
  child.on('exit', () => {
    if (managedWorkspaceMount?.child === child) managedWorkspaceMount = null;
  });

  if (await waitForMountReady(mountpoint)) {
    applyMacVolumeIcon(mountpoint);
    return { ok: true, mountpoint, error: null };
  }
  child.kill();
  return {
    ok: false,
    mountpoint: null,
    error: stderr.trim() || 'workspace mount did not become ready',
  };
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
  if (!localToken) {
    return { ok: false, endpoint, error: 'local daemon token is not initialized' };
  }

  try {
    const response = await fetch(`http://${endpoint}/rpc`, {
      method: 'POST',
      headers: {
        Accept: 'application/json',
        Authorization: `Bearer ${localToken}`,
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

type CacheTargetParams = { path?: string; node_id?: string };

type LockIdParams = { lock_id: string };

type LockExtendParams = { lock_id: string; extend_seconds: number };

async function cachePin(params: CacheTargetParams) {
  return daemonRpc('cache.pin', params);
}

async function cacheDehydrate(params: CacheTargetParams) {
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

async function lockRelease(params: LockIdParams) {
  return daemonRpc('lock.release', params);
}

async function lockExtend(params: LockExtendParams) {
  return daemonRpc('lock.extend', params);
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function rpcPayloadMethod(payload: unknown): string | null {
  if (!isRecord(payload)) return null;
  return typeof payload.method === 'string' ? payload.method : null;
}

function rpcPayloadParams(payload: unknown): Record<string, unknown> {
  if (!isRecord(payload)) return {};
  return isRecord(payload.params) ? payload.params : {};
}

function responseData(
  result: Awaited<ReturnType<typeof daemonRpc>>,
): Record<string, unknown> | null {
  const body = (result as { body?: unknown }).body;
  if (!isRecord(body)) return null;
  return isRecord(body.data) ? body.data : null;
}

async function realDirectoryPath(candidate: string): Promise<string | null> {
  const real = await fs.realpath(candidate);
  const stat = await fs.stat(real);
  return stat.isDirectory() ? real : null;
}

function candidatePath(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null;
}

async function allowedOpenRoots(): Promise<string[]> {
  const roots = new Set<string>();
  const addRoot = async (candidate: unknown) => {
    const raw = candidatePath(candidate);
    if (!raw) return;
    const real = await realDirectoryPath(raw).catch(() => null);
    if (real) roots.add(real);
  };

  const workspace = responseData(await daemonRpc('workspace.status'));
  await addRoot(workspace?.root);
  await addRoot(managedWorkspaceMount?.mountpoint);

  const mountList = responseData(await daemonRpc('mount.list'));
  const mounts = Array.isArray(mountList?.mounts) ? mountList.mounts : [];
  for (const mount of mounts) {
    if (!isRecord(mount) || mount.attached !== true) continue;
    await addRoot(mount.mount_path ?? mount.path ?? mount.mount_point);
  }

  return [...roots];
}

function isWithin(root: string, target: string): boolean {
  const relative = path.relative(root, target);
  return (
    relative === '' ||
    (relative.length > 0 && !relative.startsWith('..') && !path.isAbsolute(relative))
  );
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
ipcMain.handle('workspace:mount', mountWorkspace);
ipcMain.handle('cache:status', cacheStatus);
ipcMain.handle('cache:list', cacheList);
ipcMain.handle('cache:pin', (_event, params: CacheTargetParams) => cachePin(params));
ipcMain.handle('cache:dehydrate', (_event, params: CacheTargetParams) => cacheDehydrate(params));
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
ipcMain.handle('lock:release', (_event, params: LockIdParams) => lockRelease(params));
ipcMain.handle('lock:extend', (_event, params: LockExtendParams) => lockExtend(params));
ipcMain.handle('config:set', (_event, params: { key: string; value: string }) => configSet(params));
ipcMain.handle('app:versions', () => ({
  app: app.getVersion(),
  electron: process.versions.electron,
  chrome: process.versions.chrome,
  node: process.versions.node,
}));

// Generic read-only RPC passthrough for views that need methods outside the
// always-polled global snapshot (workset.list, mount.status, audit.events,
// snapshot.list, etc.). The main process enforces an allowlist so a compromised
// renderer cannot turn this helper into an arbitrary daemon-token proxy.
ipcMain.handle('daemon:rpc', (_event, payload: unknown) => {
  const method = rpcPayloadMethod(payload);
  if (!method || !GENERIC_RENDERER_RPC_ALLOWLIST.has(method)) {
    return {
      ok: false,
      endpoint: DAEMON_ENDPOINT,
      error: 'daemon method is not allowed from renderer rpc',
    };
  }
  return daemonRpc(method, rpcPayloadParams(payload));
});

// Open a folder in the OS file manager (Finder / Explorer / Files). Main only
// opens daemon-reported workspace/mount roots or their descendants, and only
// when the resolved target is a directory.
ipcMain.handle('shell:openPath', async (_event, target: unknown) => {
  if (typeof target !== 'string' || target.length === 0) {
    return { ok: false, error: 'invalid path' };
  }
  try {
    const targetReal = await realDirectoryPath(target);
    if (!targetReal) return { ok: false, error: 'path is not an existing directory' };
    const roots = await allowedOpenRoots();
    if (!roots.some((root) => isWithin(root, targetReal))) {
      return { ok: false, error: 'path is outside the workspace or mount roots' };
    }
    const errorMessage = await shell.openPath(targetReal);
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
      releaseChannel: string;
      autoUpdateChecks: boolean;
    }>,
  ) => {
    // Only accept known shapes; unknown keys are ignored. zoomFactor + cache
    // limit are clamped inside savePrefs so a bad value can't corrupt state.
    const safe: Partial<{
      windowChrome: 'auto' | 'native' | 'frameless';
      zoomFactor: number;
      theme: 'light' | 'dark' | 'system';
      cacheLimitGB: number | null;
      releaseChannel: ReleaseChannel;
      autoUpdateChecks: boolean;
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
    if (
      patch.releaseChannel === 'dev' ||
      patch.releaseChannel === 'nightly' ||
      patch.releaseChannel === 'alpha' ||
      patch.releaseChannel === 'beta' ||
      patch.releaseChannel === 'stable'
    ) {
      safe.releaseChannel = patch.releaseChannel;
    }
    if (typeof patch.autoUpdateChecks === 'boolean') {
      safe.autoUpdateChecks = patch.autoUpdateChecks;
    }
    const next = savePrefs(safe);
    if (safe.releaseChannel !== undefined) {
      configureAutoUpdater(next.releaseChannel);
    }
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

ipcMain.handle('updates:status', () => {
  const prefs = loadPrefs();
  configureAutoUpdater(prefs.releaseChannel);
  return getUpdateStatus(prefs.releaseChannel);
});

ipcMain.handle('updates:check', async () => {
  const prefs = loadPrefs();
  return checkForUpdates(prefs.releaseChannel);
});

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
    const prefs = loadPrefs();
    configureAutoUpdater(prefs.releaseChannel);
    await ensureManagedDaemon();
    const window = await createWindow();
    if (!IS_SMOKE) {
      void mountWorkspace();
    }

    if (prefs.autoUpdateChecks && app.isPackaged) {
      void checkForUpdates(prefs.releaseChannel);
    }

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

function unmountManagedWorkspaceSync(): void {
  const mount = managedWorkspaceMount;
  if (!mount) return;
  if (process.platform === 'darwin') {
    spawnSync('/sbin/umount', [mount.mountpoint], { stdio: 'ignore' });
  } else if (process.platform === 'linux') {
    const unmount = spawnSync('fusermount3', ['-u', mount.mountpoint], { stdio: 'ignore' });
    if (unmount.status !== 0) {
      spawnSync('fusermount', ['-u', mount.mountpoint], { stdio: 'ignore' });
    }
  }
}

app.on('before-quit', () => {
  unmountManagedWorkspaceSync();
  if (managedWorkspaceMount && !managedWorkspaceMount.child.killed) {
    managedWorkspaceMount.child.kill();
  }
  if (managedDaemon && !managedDaemon.killed) {
    managedDaemon.kill();
  }
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    app.quit();
  }
});
