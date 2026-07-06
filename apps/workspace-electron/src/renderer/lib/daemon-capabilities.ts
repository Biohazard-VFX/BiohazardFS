// Daemon capability map: which RPC methods are actually implemented vs stubbed.
//
// The daemon (per docs/architecture/DAEMON_API.md) splits its surface into:
//   - spine:    read/low-risk methods backed by the in-memory DaemonBackend.
//   - periphery: wired but returns `method_not_implemented` (destructive,
//                admin, data-moving, not-yet-built).
//
// `schema.list`/`daemon.methods` expose only method NAMES (not status), so the
// capability truth is this static spine set, kept in sync with DAEMON_API.md.
// The UI uses it to render unimplemented actions as disabled with an honest
// "requires daemon support" state, instead of letting them fire into a void and
// surface as a confusing failure. As a safety net, action handlers also treat a
// `method_not_implemented` response as "not available" rather than an error.

export const METHOD_NOT_IMPLEMENTED = 'method_not_implemented';

// Spine methods (implemented) as of the current daemon scaffold. Source of
// truth: docs/architecture/DAEMON_API.md "Spine methods".
const SPINE: ReadonlySet<string> = new Set([
  // daemon runtime
  'daemon.status',
  'daemon.health',
  'daemon.version',
  'daemon.methods',
  'daemon.events.subscribe',
  // workspace runtime
  'workspace.status',
  'workspace.list',
  // auth/session
  'auth.status',
  'auth.whoami',
  'auth.credentials_path',
  // config
  'config.path',
  'config.show',
  'config.get',
  'config.validate',
  // mount
  'mount.status',
  'mount.list',
  // file
  'file.stat',
  'file.list',
  'file.checksum',
  'file.history',
  'file.versions',
  'file.write',
  'file.read',
  // cache
  'cache.status',
  'cache.list',
  'cache.pin',
  'cache.unpin',
  'cache.hydrate',
  'cache.dehydrate',
  'cache.verify',
  // lock
  'lock.list',
  'lock.acquire',
  'lock.release',
  'lock.status',
  'lock.extend',
  // conflict
  'conflict.list',
  'conflict.show',
  // transfer
  'transfer.list',
  'transfer.status',
  // snapshot
  'snapshot.list',
  // workset
  'workset.list',
  'workset.show',
  // collaboration reads
  'invite.list',
  'share.list',
  'grant.list',
  'publish.list',
  // audit reads
  'audit.events',
  'audit.event',
  'audit.actor',
  // schema
  'schema.list',
  'schema.method',
]);

export function isSpine(method: string): boolean {
  return SPINE.has(method);
}

export function isStubbed(method: string): boolean {
  return !SPINE.has(method);
}

// True when a daemon response envelope represents "method not implemented".
// `body` may be the main-process wrapper ({body}) or a raw envelope.
export function isMethodNotImplemented(body: unknown): boolean {
  const env = body as { error?: { code?: string } } | undefined;
  return env?.error?.code === METHOD_NOT_IMPLEMENTED;
}
