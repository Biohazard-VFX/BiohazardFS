import { useEffect, useState } from 'react';

// React binding for the Electron-owned UI prefs (zoom, window chrome, theme).
// Loads once on mount and stays in sync with main via the `prefs:changed`
// broadcast (so Ctrl+± keyboard zoom is reflected in Settings live).

export type Prefs = Awaited<ReturnType<typeof window.biohazardfs.prefsGet>>;
export type AppInfo = Awaited<ReturnType<typeof window.biohazardfs.appInfo>>;
export type Theme = Prefs['theme'];

const THEME_STORAGE_KEY = 'biohazardfs.theme';

function systemPrefersDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function resolveDark(theme: Theme | undefined): boolean {
  if (theme === 'light') return false;
  if (theme === 'dark') return true;
  return systemPrefersDark(); // 'system' or unknown → follow OS
}

// Apply the theme to <html> and mirror it to localStorage. localStorage is read
// by the inline boot script in index.html so the next launch paints in the
// chosen theme before React mounts (no theme flash).
function applyTheme(theme: Theme | undefined) {
  const dark = resolveDark(theme);
  document.documentElement.classList.toggle('dark', dark);
  try {
    // Store the resolved choice the boot script understands: the literal theme
    // ('light'|'dark'|'system'). The boot script resolves 'system' itself.
    localStorage.setItem(THEME_STORAGE_KEY, theme ?? 'dark');
  } catch {
    // localStorage can be unavailable in some sandboxed contexts; the in-memory
    // class toggle still works for the session.
  }
}

export function useTheme() {
  const { prefs } = usePrefs();
  useEffect(() => {
    applyTheme(prefs?.theme);
  }, [prefs?.theme]);

  // When following the system theme, react to OS color-scheme changes live.
  useEffect(() => {
    const mql = window.matchMedia('(prefers-color-scheme: dark)');
    const onChange = () => {
      if (prefs?.theme === 'system') {
        document.documentElement.classList.toggle('dark', mql.matches);
      }
    };
    mql.addEventListener('change', onChange);
    return () => {
      mql.removeEventListener('change', onChange);
    };
  }, [prefs?.theme]);
}

export type PrefsHook = ReturnType<typeof usePrefs>;

const ZOOM_MIN = 0.5;
const ZOOM_MAX = 2;

export function usePrefs() {
  const [prefs, setPrefs] = useState<Prefs | null>(null);

  useEffect(() => {
    let cancelled = false;
    void window.biohazardfs.prefsGet().then((p) => {
      if (!cancelled) setPrefs(p);
    });
    const off = window.biohazardfs.onPrefsChanged((p) => {
      setPrefs(p);
    });
    return () => {
      cancelled = true;
      off();
    };
  }, []);

  async function setZoom(zoomFactor: number) {
    const clamped = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, Math.round(zoomFactor * 100) / 100));
    setPrefs(await window.biohazardfs.prefsSet({ zoomFactor: clamped }));
  }

  async function setWindowChrome(windowChrome: Prefs['windowChrome']) {
    setPrefs(await window.biohazardfs.prefsSet({ windowChrome }));
  }

  async function setTheme(theme: Prefs['theme']) {
    setPrefs(await window.biohazardfs.prefsSet({ theme }));
  }

  async function setCacheLimit(gb: number | null) {
    setPrefs(await window.biohazardfs.prefsSet({ cacheLimitGB: gb }));
  }

  return { prefs, setZoom, setWindowChrome, setTheme, setCacheLimit };
}

export function useAppInfo() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    let cancelled = false;
    void window.biohazardfs.appInfo().then((i) => {
      if (!cancelled) setInfo(i);
    });
    return () => {
      cancelled = true;
    };
  }, []);
  return info;
}
