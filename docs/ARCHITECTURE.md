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
