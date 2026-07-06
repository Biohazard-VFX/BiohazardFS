import { describe, expect, it } from 'vitest';

import {
  METHOD_NOT_IMPLEMENTED,
  isMethodNotImplemented,
  isSpine,
  isStubbed,
} from '../daemon-capabilities';

// Locks the capability map against the safety-critical gating: if these flip,
// the UI would either hide a working action or offer a broken one.
describe('daemon capability map', () => {
  it('flags the spine read methods as implemented', () => {
    expect(isSpine('workspace.list')).toBe(true);
    expect(isSpine('cache.pin')).toBe(true);
    expect(isSpine('cache.dehydrate')).toBe(true);
    expect(isSpine('lock.acquire')).toBe(true);
    expect(isSpine('transfer.list')).toBe(true);
    expect(isSpine('auth.whoami')).toBe(true);
    expect(isSpine('mount.status')).toBe(true);
  });

  it('flags the periphery write/admin methods as stubbed (failing-case)', () => {
    expect(isStubbed('transfer.pause')).toBe(true);
    expect(isStubbed('transfer.resume')).toBe(true);
    expect(isStubbed('conflict.preserve_all')).toBe(true);
    expect(isStubbed('conflict.resolve')).toBe(true);
    expect(isStubbed('config.set')).toBe(true);
    expect(isStubbed('mount.attach')).toBe(true);
    expect(isStubbed('cache.evict')).toBe(true);
    expect(isStubbed('admin.user.list')).toBe(true);
  });

  it('detects method_not_implemented envelopes', () => {
    expect(isMethodNotImplemented({ error: { code: METHOD_NOT_IMPLEMENTED } })).toBe(true);
    expect(isMethodNotImplemented({ error: { code: 'other_error' } })).toBe(false);
    expect(isMethodNotImplemented({})).toBe(false);
    expect(isMethodNotImplemented(undefined)).toBe(false);
  });
});
