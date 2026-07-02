# BiohazardFS Architecture Draft

```text
Electron Workspace UI
  ↓ local IPC/API
Rust daemon: biohazardfsd
  ↓
Filesystem adapter: FUSE / WinFsp / Cloud Files / File Provider
  ↓
Local cache + state DB
  ↓
BiohazardFS control plane
  ↓
S3-compatible object storage + PostgreSQL metadata
```

Electron is a shell. Rust is the sync/filesystem engine.

## Daemon API boundary

The local daemon API is a product contract. See `docs/DAEMON_API.md`.

Key decisions:

- Artist machines run `biohazardfsd` as a per-user daemon that auto-starts at login.
- Primary local transport is platform IPC: Unix domain socket on Linux/macOS, named pipe on Windows.
- Optional loopback HTTP is allowed for dev/test/integration mode only.
- Local clients authenticate through same-OS-user ownership plus an owner-only local session token.
- The daemon uses JSON-RPC-like method calls for IPC and optional HTTP.
- The daemon exposes a structured one-way event stream first: NDJSON/SSE; bidirectional streaming can be added later.
- The daemon stores local operational state in an owner-only SQLite DB.
- Normal CLI/Electron/MCP operations go through the daemon for mount/cache/local state.
- Explicit direct-server/headless/admin mode is allowed only for operations that do not require local mount/cache/filesystem state.
- The daemon supports full optimistic offline mode and preserves both sides as conflicts after divergent reconnects.

## Versioning model

BiohazardFS is not a Git repository mounted as a drive. The live filesystem core should follow a virtual filesystem model:

```text
virtual filesystem
+ local cache/pinning
+ distributed file locks
+ immutable file versions
+ append-only event journal
+ point-in-time snapshots
+ read-only snapshot browsing/restores
```

Git/Git LFS may be supported as optional import/export or for code/manifests/templates, but it is not the primary live storage engine.

### Why not Git as core?

- Per-save commits create noisy, misleading history.
- Huge VFX working trees can make Git status/index operations expensive.
- Git LFS helps with bytes but does not solve live virtual filesystem UX.
- Binary DCC files need locks and conflict copies, not merges.
- BiohazardFS needs hydrate/dehydrate, cache state, device revocation, permissions, and share links outside Git's model.

### Required audit/version primitives

- `FileVersion`: immutable committed file content/metadata version.
- `EventJournal`: append-only audit trail of create/write/rename/delete/restore/lock/share/publish operations.
- `Snapshot`: point-in-time view of a project/workset/filesystem tree.
- `FileLock`: distributed lock with owner/device/expiry/admin override.
- `Conflict`: preserved divergent versions with clear user-visible state.

Every event should include provenance: actor, device, source (`ui`, `cli`, `agent`, `api`, `server`), timestamp, affected paths/IDs, and request/correlation ID.

## Packaging/release boundary

Packaging and release behavior is a product contract. See `docs/PACKAGING.md`.

Key decisions:

- Public/open-source distribution discipline starts from the beginning.
- Primary distribution is one platform-native installer per OS.
- The desktop installer installs the desktop app, CLI, daemon, autostart service, and required integration helpers.
- Use one product version across desktop app, CLI, daemon, and server/control-plane artifacts at first.
- Release channels are `dev`, `nightly`, `alpha`, `beta`, and `stable`.
- Default artist install uses a per-user daemon that auto-starts at login.
- Uninstall preserves cache/config/user data unless explicit purge/remove-data is selected.
- Code signing/notarization is required before serious public/stable distribution, but not earliest MVP/dev artifacts.

## Filesystem/cache semantics boundary

Filesystem and cache semantics are product contracts. See `docs/FILESYSTEM_SEMANTICS.md`.

Key decisions:

- Preserve filename case, but enforce case-insensitive sibling uniqueness by default.
- Deleting inside the mount means server/cloud trash, not local cache removal.
- Local cache removal is explicit dehydrate/uncache behavior.
- Writes are durable locally before upload/commit; server versions commit only after integrity verification.
- MVP hydrates full files before allowing normal open/edit workflows.
- Dirty/unuploaded and pinned files are never auto-evicted.
- Cache-full behavior pauses downloads and blocks/fails new writes safely before data loss.
- Lock conflicts block writes where possible; otherwise user work is preserved as conflict copies.
- Full optimistic offline mode records base state and creates conflicts on divergent reconnect.
- Symlinks are supported but constrained to authorized roots unless policy allows otherwise.
- Image sequences are normal files in v1 with listing/prefetch optimization, not special version semantics.

## Server metadata boundary

The server/control-plane metadata schema is a product contract. See `docs/METADATA_SCHEMA.md`.

Key decisions:

- Include an org/studio boundary from day one.
- Use stable `node_id` identity for filesystem nodes; paths are derived from mutable parent/name.
- File versions point to content manifest references; individual chunks do not need DB rows in v1.
- Snapshots support org, project, workset, and subtree scopes.
- Grants can attach to projects, worksets, nodes, and shares.
- Locks attach to node IDs where possible, with path snapshots and provisional IDs for offline-created files.
- Offline operations are first-class server records for replay/reconciliation.
- Deletes use trash records, soft-deleted nodes, and retention/purge policy.
- Audit events use indexed envelope columns plus typed schema-versioned JSON payloads.
