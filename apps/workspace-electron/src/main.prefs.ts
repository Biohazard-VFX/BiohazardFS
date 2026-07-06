import { app } from 'electron';
import fs from 'node:fs';
import path from 'node:path';

// Electron-owned UI preferences (NOT daemon config). Zoom factor and window
// chrome are presentation; per CLAUDE.md the daemon owns sync/runtime truth and
// Electron owns presentation. Persisted as plain JSON in userData so it is
// owner-only, human-readable, and trivially reset by deleting the file.

export type WindowChrome = 'auto' | 'native' | 'frameless';
export type Theme = 'light' | 'dark' | 'system';
export type ReleaseChannel = 'dev' | 'nightly' | 'alpha' | 'beta' | 'stable';

export type Prefs = {
  windowChrome: WindowChrome;
  zoomFactor: number;
  theme: Theme;
  // Local cache size preference in GB, or null for no limit. This is saved for
  // future daemon quota support; the desktop app does not enforce it yet.
  cacheLimitGB: number | null;
  releaseChannel: ReleaseChannel;
  autoUpdateChecks: boolean;
};

function isReleaseChannel(value: unknown): value is ReleaseChannel {
  return (
    value === 'dev' ||
    value === 'nightly' ||
    value === 'alpha' ||
    value === 'beta' ||
    value === 'stable'
  );
}

function releaseChannelFromMetadata(): ReleaseChannel | null {
  if (!app.isPackaged) return null;
  try {
    const metadataPath = path.join(process.resourcesPath, 'release-metadata.json');
    const metadata = JSON.parse(fs.readFileSync(metadataPath, 'utf8')) as { channel?: unknown };
    return isReleaseChannel(metadata.channel) ? metadata.channel : null;
  } catch {
    return null;
  }
}

const DEFAULT_RELEASE_CHANNEL: ReleaseChannel =
  releaseChannelFromMetadata() ?? (app.isPackaged ? 'stable' : 'dev');

const DEFAULTS: Prefs = {
  windowChrome: 'auto',
  zoomFactor: 1,
  theme: 'dark',
  cacheLimitGB: null,
  releaseChannel: DEFAULT_RELEASE_CHANNEL,
  autoUpdateChecks: false,
};

export const ZOOM_MIN = 0.5;
export const ZOOM_MAX = 2;
export const ZOOM_STEP = 0.1;

let prefsPath: string | null = null;
let cache: Prefs | null = null;

function prefsFilePath(): string {
  if (!prefsPath) {
    prefsPath = path.join(app.getPath('userData'), 'prefs.json');
  }
  return prefsPath;
}

export function loadPrefs(): Prefs {
  if (cache) return cache;
  try {
    const raw = fs.readFileSync(prefsFilePath(), 'utf8');
    const parsed = JSON.parse(raw) as Partial<Prefs>;
    cache = {
      windowChrome:
        parsed.windowChrome === 'native' ||
        parsed.windowChrome === 'frameless' ||
        parsed.windowChrome === 'auto'
          ? parsed.windowChrome
          : DEFAULTS.windowChrome,
      zoomFactor: clampZoom(parsed.zoomFactor),
      theme:
        parsed.theme === 'light' || parsed.theme === 'dark' || parsed.theme === 'system'
          ? parsed.theme
          : DEFAULTS.theme,
      cacheLimitGB: clampCacheLimit(parsed.cacheLimitGB),
      releaseChannel: clampReleaseChannel(parsed.releaseChannel),
      autoUpdateChecks:
        typeof parsed.autoUpdateChecks === 'boolean'
          ? parsed.autoUpdateChecks
          : DEFAULTS.autoUpdateChecks,
    };
  } catch {
    // Missing/unreadable prefs → fall back to defaults. Never throw on prefs;
    // the UI must still come up.
    cache = { ...DEFAULTS };
  }
  return cache;
}

export function savePrefs(patch: Partial<Prefs>): Prefs {
  const next: Prefs = {
    windowChrome: patch.windowChrome ?? loadPrefs().windowChrome,
    zoomFactor:
      patch.zoomFactor === undefined ? loadPrefs().zoomFactor : clampZoom(patch.zoomFactor),
    theme: patch.theme ?? loadPrefs().theme,
    cacheLimitGB:
      patch.cacheLimitGB === undefined
        ? loadPrefs().cacheLimitGB
        : clampCacheLimit(patch.cacheLimitGB),
    releaseChannel:
      patch.releaseChannel === undefined
        ? loadPrefs().releaseChannel
        : clampReleaseChannel(patch.releaseChannel),
    autoUpdateChecks:
      patch.autoUpdateChecks === undefined
        ? loadPrefs().autoUpdateChecks
        : Boolean(patch.autoUpdateChecks),
  };
  cache = next;
  try {
    fs.writeFileSync(prefsFilePath(), JSON.stringify(next, null, 2), 'utf8');
  } catch {
    // Persistence failure is non-fatal for the session; the pref still applies
    // in-memory until the app is closed.
  }
  return next;
}

export function clampZoom(value: unknown): number {
  const n = typeof value === 'number' && Number.isFinite(value) ? value : DEFAULTS.zoomFactor;
  return Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(n * 100) / 100));
}

// Cache limit: a positive GB value, or null (no limit). Anything else → default.
export function clampCacheLimit(value: unknown): number | null {
  if (value === null) return null;
  const n = typeof value === 'number' && Number.isFinite(value) ? value : NaN;
  if (Number.isNaN(n) || n <= 0) return DEFAULTS.cacheLimitGB;
  return Math.round(n);
}

export function clampReleaseChannel(value: unknown): ReleaseChannel {
  return isReleaseChannel(value) ? value : DEFAULTS.releaseChannel;
}

// Resolve whether the window should be frameless for a given platform.
// `auto` = frameless on Linux (Wayland/X clients that already provide their own
// window management), native decorations on macOS/Windows.
export function resolveFrameless(prefs: Prefs, platform: NodeJS.Platform): boolean {
  switch (prefs.windowChrome) {
    case 'frameless':
      return true;
    case 'native':
      return false;
    case 'auto':
    default:
      return platform === 'linux';
  }
}
