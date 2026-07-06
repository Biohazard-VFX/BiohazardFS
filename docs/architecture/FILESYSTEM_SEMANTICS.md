# BiohazardFS Filesystem and Cache Semantics

Status: draft reference
Audience: filesystem adapter implementers, daemon implementers, CLI/agent authors, QA, operators

This document defines the safety rules for the mounted workspace, local cache, writes, deletes, locks, conflicts, offline behavior, and crash recovery.

The priority order is:

1. Never lose user data.
2. Never silently overwrite divergent work.
3. Keep cloud/server delete separate from local cache removal.
4. Prefer boring, predictable semantics over clever sync behavior.
5. Expose machine-readable state for agents and diagnostics.

## Core decisions

- Preserve filename case, but enforce case-insensitive sibling uniqueness by default.
- Deleting inside the mount means server/cloud trash, not local cache removal.
- Local cache removal is explicit: “Remove local copy” / dehydrate / uncache.
- Writes are durable locally before upload/commit.
- Server versions commit only after full content integrity verification.
- MVP hydrates full files before opening for normal DCC/file-editing workflows.
- Dirty/unuploaded and pinned files are never evicted automatically.
- Cache-full behavior pauses downloads and blocks/fails new writes safely before data loss.
- If a file is locked by someone else, block normal write/open-for-write where possible.
- If a lock cannot be enforced, preserve user work as a conflict copy.
- Full optimistic offline mode is allowed, but reconnect divergence always creates conflicts.
- Symlinks are supported but must not escape authorized roots unless explicitly allowed by policy.
- Image sequences are normal files in v1, with listing/prefetch optimizations but no special version semantics.

## Path and namespace rules

### Path normalization

The daemon must normalize paths before policy checks and metadata lookup.

Rules:

- Reject empty paths for operations that require a target.
- Reject path traversal outside authorized mount roots.
- Reject embedded NUL/control characters.
- Normalize repeated separators.
- Treat `.` segments as no-ops.
- Reject or canonicalize `..` segments before authorization.
- Preserve user-visible case.
- Store/display normalized absolute workspace paths in audit/path snapshots.

### Case sensitivity

BiohazardFS preserves case but enforces case-insensitive sibling uniqueness by default.

Allowed:

```text
/Project/Shot010/Plate.exr
/Project/Shot020/plate.exr
```

Rejected in the same directory:

```text
/Project/Shot010/Plate.exr
/Project/Shot010/plate.exr
```

Reason:

- Windows and common macOS filesystems are case-insensitive by default.
- Cross-platform artists must see one consistent namespace.
- Case-only rename must be handled as an explicit rename operation with safe platform-specific behavior.

### Node identity

The mounted path is not the identity. Server metadata uses stable `node_id` identity.

Rules:

- Rename/move preserves `node_id`.
- Audit includes both stable IDs and path snapshots.
- Cache records should key by stable IDs when available.
- Offline-created files may use provisional local IDs until the server assigns `node_id`.

## File and directory states

User-visible states should be representable in CLI/daemon JSON and UI badges.

Minimum file states:

```text
placeholder       visible metadata, no local content
hydrating         content downloading into cache
cached            complete local content available
pinned            cached and not auto-evictable
modified_local    local dirty content not fully committed to server
uploading         dirty content uploading/committing
synced            local cached content matches current server version
offline_queued    local mutation queued while offline
conflicted        conflict exists for this node/path
locked            locked by current user/device
locked_remote     locked by someone else
error             requires user/agent attention
```

Directory states are derived from child states and explicit workset/cache policy:

```text
available
partially_cached
fully_cached
pinned
has_dirty
has_conflicts
has_errors
```

## Hydration behavior

MVP hydrates full files before allowing normal open/edit workflows.

Rules:

- Opening a placeholder for read triggers hydration.
- Opening a file for write triggers hydration first unless creating a new file.
- Hydration writes into a temporary cache object and atomically promotes it to cached state after verification.
- Partial/failed hydration must not replace a known-good cached file.
- Future streaming for video/media workloads may be added, but must not weaken DCC write safety.

Prefetch heuristics may include:

- nearby image sequence frames
- sibling sidecar files
- small project metadata files
- recently opened folders/files
- assigned workset roots

Prefetch is advisory and must yield to explicit user/agent actions and cache safety.

## Write lifecycle

Local writes are durable first, then uploaded/committed asynchronously.

Write path:

1. Filesystem adapter receives write/create/rename/delete request.
2. Daemon validates access, lock state, mount state, cache space, and local policy.
3. Dirty data is written to durable local cache/state.
4. Local state DB records dirty operation before reporting durable success where possible.
5. Transfer manager uploads content/metadata.
6. Server verifies content integrity.
7. Server creates immutable `FileVersion` and updates node current version.
8. Daemon marks local state synced after server acknowledgment.

Rules:

- Dirty/unuploaded data must survive daemon restart and OS reboot.
- A server-visible file version is not committed until content integrity is verified.
- Failed uploads remain dirty/queued and visible to UI/CLI/agents.
- Daemon must expose enough state for agents to detect unsynced work.
- Server commit/audit provenance includes actor, device, source, request/operation IDs, node ID, version ID, and path snapshot.

## Atomic writes and DCC behavior

DCC and creative apps commonly write through temp files, atomic rename, sidecars, autosaves, lock files, and bursts of small metadata updates.

Required behavior:

- Support create-write-close-rename workflows.
- Treat rename over existing files as data-moving and conflict-prone.
- Preserve temp/autosave files unless ignored by explicit policy.
- Do not assume a close event means the file is semantically ready to publish.
- Do not publish automatically unless configured by workflow/policy.
- Upload ordinary saves; publish/version labels are separate product actions.

Recommended MVP implementation:

- Commit normal file versions on completed durable writes.
- Treat explicit `publish` as a labeled/audited milestone over a specific version.
- Use app/DCC behavior matrix tests before claiming support for specific applications.

## Deletes, trash, and dehydrate

Cloud/server delete and local cache removal are separate operations.

### Delete in mounted workspace

Deleting a file/folder inside the mount means server/cloud trash.

Rules:

- Delete creates a trash record and soft-deletes the node.
- Delete is audited.
- Delete is reversible until purge/retention removes it.
- Delete is data-moving/destructive under CLI/daemon safety policy.
- Deleting dirty/unuploaded local work must either block or preserve a recoverable local version before trashing.

### Remove local copy / dehydrate

Dehydrate removes local cached content without deleting server/cloud data.

Rules:

- Dehydrate never deletes server data.
- Dehydrate must not remove dirty/unuploaded content.
- Dehydrate must not remove pinned content unless explicitly unpinned or forced by authorized action.
- Dehydrate leaves a placeholder and metadata visible if authorized.

## Cache policy

### Cache quota

The user chooses cache location during setup. The daemon tracks cache quota and actual usage.

Rules:

- Default cache limit should leave a safety buffer on the selected volume.
- Pinned files count against quota but are not auto-evictable.
- Dirty/unuploaded files are never auto-evictable.
- Cache policy must be queryable by CLI/daemon API.

### Eviction order

Automatic eviction may remove only safe cached content.

Recommended eviction priority:

1. Unpinned, synced, least-recently-used files outside active worksets.
2. Unpinned, synced files in inactive worksets.
3. Unpinned, synced files in active worksets if user policy allows.

Never auto-evict:

- dirty/unuploaded files
- files with pending local operations
- pinned files
- files currently open/in use
- files required for active transfer verification

### Cache-full behavior

Disk full must be annoying, not corrupting.

Rules:

- Pause downloads before exhausting disk.
- Block or fail new large writes safely if the daemon cannot reserve enough local space.
- Surface `cache_full` errors through daemon/CLI/UI.
- Preserve dirty data even if that means all new downloads stop.
- Provide actionable remediation: dehydrate safe files, move cache, increase quota, unpin files.

## Lock enforcement

Locks protect binary/scene files and other unmergeable assets.

Rules:

- If a file is locked by another user/device, block normal write/open-for-write where platform adapter can enforce it.
- Display lock state in UI/CLI/filesystem metadata where possible.
- Lock checks must happen before write/rename/delete when online.
- Offline operations may proceed only under optimistic offline policy and must reconcile later.
- If lock enforcement is bypassed or impossible, preserve local work as a conflict copy.
- Breaking a lock is admin/destructive and audited.

Lock identity:

- Existing files: lock by `node_id`.
- Offline-created files: provisional local ID until server node ID assignment.
- Path snapshot is display/audit metadata, not primary identity.

## Conflict behavior

Conflicts preserve every version and never silently overwrite.

Conflict triggers include:

- concurrent writes to same base version
- offline local write plus remote write
- rename/rename divergence
- delete/write divergence
- lock violation or stale lock conflict
- permission/workset changes racing with local operations

Visible conflict copy naming should be boring and understandable:

```text
scene.nk
scene (conflict - nicholai - workstation - 2026-07-02).nk
```

Rules:

- Conflict records are the source of truth; conflict filenames are user-facing recovery aids.
- Conflict copies must preserve actor/device/source/timestamp metadata.
- Agents should use conflict IDs from daemon/server APIs instead of parsing filenames.
- Conflict resolution is data-moving and requires stricter safety in agent-safe mode.
- Automatic merge of divergent file content is out of scope for MVP.

## Offline behavior

BiohazardFS supports full optimistic offline mode.

Rules:

- Known authorized namespace remains visible with degraded/offline state.
- Local changes queue durably in the daemon state DB.
- Offline operations record base node/version/snapshot state.
- Offline creates use provisional local IDs.
- Reconnect submits first-class operation records to the server.
- Server detects base divergence and creates conflicts.
- Reconnect never silently overwrites remote changes.
- Offline audit events are marked local then server-acknowledged after replay.

Offline user experience:

- Show offline/degraded state clearly.
- Show queued operation count.
- Show dirty/unsynced files.
- Make reconnect/reconciliation progress visible.
- Provide agent-readable queue and conflict state.

## Symlink behavior

Symlinks are supported but constrained.

Rules:

- Symlink nodes are represented explicitly in metadata.
- Symlink targets must not escape authorized roots unless policy explicitly permits it.
- Cross-platform clients must expose unsupported symlink cases as clear errors, not silent copies.
- Symlink creation and resolution are audited where security-relevant.
- Agents must be able to identify symlinks via file metadata.

## Image sequences and huge directories

Image sequences are normal files in v1.

Rules:

- Do not create special version semantics for image sequences in MVP.
- Optimize listing/pagination for huge directories.
- Prefetch nearby frames opportunistically when opening a frame.
- Avoid loading entire huge directories into agent/CLI output by default.
- Use pagination, field masks, and streaming output for large listings.

## Crash recovery

The daemon must recover safely from crashes and reboots.

On startup, daemon recovery should:

1. Open and migrate local SQLite state DB if needed.
2. Reconcile in-progress local writes and temp cache objects.
3. Preserve dirty/unuploaded data.
4. Resume or mark transfers retryable.
5. Rebuild cache indexes from durable metadata where needed.
6. Reconnect event stream clients through resync-capable list/status APIs.
7. Surface any uncertain state as `error` or `needs_attention` rather than guessing.

Rules:

- Unknown dirty state must never be deleted automatically.
- Partial uploads must be retried or abandoned only after verifying no server commit was created.
- Support bundles must include redacted recovery/problem records.

## Permissions and visibility

Rules:

- Unauthorized folders are hidden by default.
- If a previously visible node becomes unauthorized, clients must stop exposing content after policy refresh.
- Dirty local work created while previously authorized must be preserved and handled through admin/support workflow if access is revoked before upload.
- Agents must be able to query effective permissions for a path/node.

## Minimum invariants

- Server delete is not cache dehydrate.
- Dehydrate never deletes server data.
- Dirty/unuploaded files are never auto-evicted.
- Pinned files are never auto-evicted without explicit unpin/force policy.
- Server-visible file versions are immutable.
- New server versions require verified content integrity.
- Divergence creates conflicts, never silent overwrites.
- Case-insensitive sibling uniqueness is enforced by default.
- Local durable state is written before reporting queued/saved success where possible.
- Agents can inspect state instead of guessing from filenames or logs.

## First implementation slice

The first implementation proves safety before broad filesystem support. Current status:

1. Normalize and validate paths — IMPLEMENTED in `core::path` (`normalize_relative_path`, `validate_file_name`).
2. Enforce case-insensitive sibling uniqueness — IMPLEMENTED via `case_insensitive_sibling_key`, enforced by `daemon.file.write` and the FUSE `create` path.
3. Model placeholder/cached/pinned/dirty/conflicted states — IMPLEMENTED as the `core::cache::CacheState` machine, where `Dirty -> Evicting` and `Dirty -> Absent` are rejected at the transition layer.
4. Implement full-file hydrate into cache — IMPLEMENTED in `WorkspaceFs`: `open` fetches the whole file via `daemon.file.read` before reply. Verification is partial: the daemon-supplied `content_hash` is stored on the cache entry but not independently recomputed from the hydrated bytes.
5. Implement dehydrate that cannot remove dirty/pinned data — IMPLEMENTED. The `cache.dehydrate` spine refuses dirty and pinned entries and routes `Ready -> Evicting -> Absent` through the legal transition path.
6. Implement durable dirty-state records in local SQLite — SCAFFOLD. Dirty state lives in the in-memory `DaemonBackend` only; it does not survive daemon restart.
7. Implement cache quota checks and `cache_full` error path — SCAFFOLD. `CacheStats` carries a `quota_bytes` field but the daemon surfaces it as `None`; there is no quota enforcement and no `cache_full` path yet.
8. Implement lock-state checks in mock write path — IMPLEMENTED at the daemon `lock.*` spine (`list`/`acquire`/`release`/`status`/`extend`, with lazy expiry). The FUSE write path does not yet consult lock state before committing a write.
9. Implement conflict record creation for divergent writes — SCAFFOLD. `daemon.file.write` rejects a stale `base_version_id` with `version_conflict`, and `conflict.list`/`show` expose conflicts against the in-memory store, but no path yet creates a `Conflict` record on divergence.
10. Implement crash/restart recovery tests for dirty local state — PLANNED.

Beyond this slice, the `WorkspaceFs` write path is IMPLEMENTED: `create`, `write`, `flush`, and `fsync` buffer per-handle and push one complete blob per flush through `daemon.file.write`, with dirty-data-never-lost enforced by restoring the buffer and returning `EIO` whenever the daemon does not acknowledge the version. Directory and symlink mutations are SCAFFOLD-boundary: `mkdir` and `symlink` return `EROFS`, and `unlink`, `rmdir`, and `rename` return `EIO`, pending promotion of `daemon.file.delete` / `daemon.file.move` and FUSE-side operation-token minting for destructive and data-moving methods.

Do not claim MVP write support until dirty data survives daemon restart, cache-full behavior is safe, deletes go to trash, and divergent writes preserve all versions.
