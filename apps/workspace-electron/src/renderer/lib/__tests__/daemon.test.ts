import { describe, expect, it } from 'vitest';

import {
  DIRTY_STATES,
  asNumber,
  asString,
  cacheEntryTarget,
  computeProgress,
  dirtyEntryCount,
  entryList,
  extractData,
  extractError,
  isDirtyEntry,
  isPinnedEntry,
  keepLastGood,
  mountAttached,
  mountPathFromList,
  stateLabel,
} from '../daemon';
import { formatBytes } from '../format';

// These tests lock the safety-critical invariants the UI relies on. If any of
// them ever flips, a sync/corruption bug ships with it — fail loudly here.

describe('isDirtyEntry', () => {
  it('flags every DIRTY_STATES value', () => {
    for (const state of DIRTY_STATES) {
      expect(isDirtyEntry({ state })).toBe(true);
    }
  });

  it('flags an explicit dirty boolean', () => {
    expect(isDirtyEntry({ dirty: true, state: 'synced' })).toBe(true);
  });

  it('does not flag a clean synced file (failing-case: must stay false)', () => {
    expect(isDirtyEntry({ dirty: false, state: 'synced' })).toBe(false);
    expect(isDirtyEntry({ state: 'cached' })).toBe(false);
    expect(isDirtyEntry({})).toBe(false);
  });

  it('treats empty/missing state as not dirty', () => {
    expect(isDirtyEntry({ state: '' })).toBe(false);
    expect(isDirtyEntry({ state: undefined })).toBe(false);
  });
});

describe('dirtyEntryCount', () => {
  it('prefers the daemon dirty_entries field and supports legacy dirty_count', () => {
    expect(dirtyEntryCount({ dirty_entries: 3, dirty_count: 1 })).toBe(3);
    expect(dirtyEntryCount({ dirty_count: 2 })).toBe(2);
    expect(dirtyEntryCount({})).toBeNull();
  });
});

describe('cacheEntryTarget', () => {
  it('uses path when present, otherwise falls back to node_id', () => {
    expect(cacheEntryTarget({ path: 'shots/a.exr', node_id: 'node_a' })).toEqual({
      path: 'shots/a.exr',
    });
    expect(cacheEntryTarget({ node_id: 'node_a' })).toEqual({ node_id: 'node_a' });
    expect(cacheEntryTarget({})).toBeNull();
  });
});

describe('mount helpers', () => {
  it('reads attached and mount_path from daemon mount payloads', () => {
    expect(mountAttached({ attached: true })).toBe(true);
    expect(mountAttached({ attached: false })).toBe(false);
    expect(mountAttached({ mounted: true })).toBe(true);
    expect(mountAttached({ mounted: false })).toBe(false);
    expect(mountPathFromList({ mounts: [{ mount_path: '/mnt/biohazard', attached: true }] })).toBe(
      '/mnt/biohazard',
    );
  });
});

describe('isPinnedEntry', () => {
  it('honors pinned boolean and pinned state vocabulary', () => {
    expect(isPinnedEntry({ pinned: true })).toBe(true);
    expect(isPinnedEntry({ state: 'pinned' })).toBe(true);
    expect(isPinnedEntry({ state: 'cached_pinned' })).toBe(true);
  });

  it('does not pin an ordinary cached file', () => {
    expect(isPinnedEntry({ state: 'cached' })).toBe(false);
    expect(isPinnedEntry({})).toBe(false);
  });
});

describe('keepLastGood', () => {
  it('replaces when the next envelope carries a body', () => {
    const prev = { ok: true, endpoint: 'e', body: { old: true } };
    const next = { ok: true, endpoint: 'e', body: { new: true } };
    expect(keepLastGood(prev, next)).toBe(next);
  });

  it('keeps previous on transport dropout (body undefined)', () => {
    const prev = { ok: true, endpoint: 'e', body: { old: true } };
    const next = { ok: false, endpoint: 'e', error: 'boom' };
    expect(keepLastGood(prev, next)).toBe(prev);
  });

  it('falls back to next when there is no previous good state', () => {
    const next = { ok: false, endpoint: 'e', error: 'boom' };
    expect(keepLastGood(null, next)).toBe(next);
  });
});

describe('computeProgress', () => {
  it('reads a 0..1 ratio', () => {
    expect(computeProgress({ progress: 0.5 })).toEqual({ percent: 50, label: '50%' });
    expect(computeProgress({ progress: 0 })).toEqual({ percent: 0, label: '0%' });
    expect(computeProgress({ progress: 1 })).toEqual({ percent: 100, label: '100%' });
  });

  it('reads a >1 percentage value', () => {
    expect(computeProgress({ progress: 42 })).toEqual({ percent: 42, label: '42%' });
  });

  it('derives from bytes_done / bytes_total when progress is absent', () => {
    expect(computeProgress({ bytes_done: 250, bytes_total: 1000 })).toEqual({
      percent: 25,
      label: '25%',
    });
  });

  it('clamps over-complete byte ratios to 100%', () => {
    expect(computeProgress({ bytes_done: 2000, bytes_total: 1000 }).percent).toBe(100);
  });

  it('falls back to the em-dash placeholder when nothing is known', () => {
    expect(computeProgress({})).toEqual({ percent: 0, label: '—' });
  });
});

describe('formatBytes', () => {
  it('scales across unit boundaries', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1023)).toBe('1023 B');
    expect(formatBytes(1024)).toBe('1.00 KB');
    expect(formatBytes(1048576)).toBe('1.00 MB');
    expect(formatBytes(1073741824)).toBe('1.00 GB');
  });

  it('reports unknown for null/missing/negative', () => {
    expect(formatBytes(null)).toBe('unknown');
    expect(formatBytes(undefined)).toBe('unknown');
    expect(formatBytes(-1)).toBe('unknown');
    expect(formatBytes('not a number')).toBe('unknown');
  });
});

describe('extractData / extractError', () => {
  it('unwraps data defensively', () => {
    expect(extractData({ ok: true, endpoint: 'e', body: { data: { a: 1 } } })).toEqual({
      a: 1,
    });
    expect(extractData({ ok: true, endpoint: 'e', body: { error: { code: 'x' } } })).toBeNull();
    expect(extractData(null)).toBeNull();
  });

  it('surfaces daemon error envelopes and transport errors', () => {
    expect(
      extractError({ ok: false, endpoint: 'e', body: { error: { code: 'nope', message: 'bad' } } }),
    ).toEqual({ code: 'nope', message: 'bad' });
    expect(extractError({ ok: false, endpoint: 'e', error: 'connection refused' })).toEqual({
      code: 'unreachable',
      message: 'connection refused',
    });
    expect(extractError(null)).toBeNull();
  });
});

describe('entryList', () => {
  it('picks the first array field present', () => {
    expect(entryList({ entries: [{ a: 1 }] }, ['entries', 'items'])).toEqual([{ a: 1 }]);
    expect(entryList({ items: [{ b: 2 }] }, ['entries', 'items'])).toEqual([{ b: 2 }]);
    expect(entryList({}, ['entries'])).toEqual([]);
    expect(entryList({ entries: 'not-an-array' }, ['entries'])).toEqual([]);
  });
});

describe('asNumber / asString', () => {
  it('coerces defensively', () => {
    expect(asNumber(3)).toBe(3);
    expect(asNumber('3.5')).toBe(3.5);
    expect(asNumber('x')).toBeNull();
    expect(asNumber(NaN)).toBeNull();
    expect(asNumber(null)).toBeNull();
    expect(asString('hi')).toBe('hi');
    expect(asString('', 'fallback')).toBe('fallback');
    expect(asString(undefined, 'fb')).toBe('fb');
  });
});

describe('stateLabel', () => {
  it('maps daemon states to artist-facing labels', () => {
    expect(stateLabel('running')).toBe('Syncing');
    expect(stateLabel('queued')).toBe('Queued');
    expect(stateLabel('pending')).toBe('Queued');
    expect(stateLabel('synced')).toBe('Done');
    expect(stateLabel('completed')).toBe('Done');
    expect(stateLabel('failed')).toBe('Failed');
    expect(stateLabel('paused')).toBe('Paused');
    expect(stateLabel('cancelled')).toBe('Cancelled');
    expect(stateLabel('')).toBe('Queued');
    expect(stateLabel('something_unknown')).toBe('something_unknown');
  });
});
