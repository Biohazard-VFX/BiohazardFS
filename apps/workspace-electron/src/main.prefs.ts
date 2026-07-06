import { app } from 'electron';
import fs from 'node:fs';
import path from 'node:path';

// Electron-owned UI preferences (NOT daemon config). Zoom factor and window
// chrome are presentation; per CLAUDE.md the daemon owns sync/runtime truth and
// Electron owns presentation. Persisted as plain JSON in userData so it is
// owner-only, human-readable, and trivially reset by deleting the file.

export type WindowChrome = 'auto' | 'native' | 'frameless';
export type Theme = 'light' | 'dark' | 'system';

export type Prefs = {
  windowChrome: WindowChrome;
  zoomFactor: number;
  theme: Theme;
  // Local cache size cap in GB, or null for no limit. Lives here (not daemon
  // config) because config.set is stubbed and a cache cap is presentation/
  // policy the desktop app enforces client-side for now.
  cacheLimitGB: number | null;
};

const DEFAULTS: Prefs = {
  windowChrome: 'auto',
  zoomFactor: 1,
  theme: 'dark',
  cacheLimitGB: null,
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
