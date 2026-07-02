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
