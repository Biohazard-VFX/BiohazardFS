import { asNumber } from './daemon';

// Human-readable byte formatting. Mirrors the original renderer behavior:
// unknown/negative → "unknown"; scales to the largest fitting unit.
export function formatBytes(value: unknown): string {
  const bytes = asNumber(value);
  if (bytes === null || bytes < 0) {
    return 'unknown';
  }
  if (bytes < 1024) {
    return `${String(Math.round(bytes))} B`;
  }
  const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
  let scaled = bytes / 1024;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  const digits = scaled >= 100 ? 0 : scaled >= 10 ? 1 : 2;
  return `${scaled.toFixed(digits)} ${units[index]}`;
}

// Compact integer with thousands separators.
export function formatCount(value: unknown): string {
  const n = asNumber(value);
  if (n === null) {
    return '—';
  }
  return Math.round(n).toLocaleString('en-US');
}

// Best-effort relative time for conflict/lock timestamps. The daemon's draft
// timestamps are freeform strings; fall back to the raw string when we cannot
// parse them. Never throw on a malformed value.
export function formatRelativeTime(value: unknown): string | null {
  const raw = typeof value === 'string' ? value : '';
  if (!raw) {
    return null;
  }
  const parsed = Date.parse(raw);
  if (Number.isNaN(parsed)) {
    return raw;
  }
  const then = parsed;
  const now = Date.now();
  const seconds = Math.round((now - then) / 1000);
  if (seconds < 0) {
    // Future-dated; show absolute.
    return new Date(then).toLocaleString('en-US');
  }
  if (seconds < 60) return 'just now';
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${String(minutes)}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${String(hours)}h ago`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${String(days)}d ago`;
  return new Date(then).toLocaleDateString('en-US');
}
