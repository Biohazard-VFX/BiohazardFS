# BiohazardFS Roadmap Draft

Status legend: **IMPLEMENTED** (real behavior, tested), **SCAFFOLD** (typed + wired + tested seam, in-memory/mock backend or returns `method_not_implemented` / `operation_not_implemented`), **PLANNED** (in the spec, not yet in code).

1. ADRs and skeleton. — **IMPLEMENTED.** ADR `0001-repository-layout` is accepted; the Rust workspace (core, api-types, cli, daemon, fuse, server), the Layer 0 method/operation/command registry shared across all three surfaces, the Electron shell, Docker/Compose/Helm scaffolds, and CI gates are in place.
2. Linux read-only FUSE prototype. — **IMPLEMENTED.** `biohazardfs-fuse mount` exposes a source workspace tree read-only via `ReadOnlyWorkspaceFs`, verified by a live-mount smoke (`scripts/ci/fuse-smoke.sh`) that checks read and rejects write.
3. Hydration/cache prototype. — **SCAFFOLD.** Cache state machine plus `cache.status/list/pin/unpin/hydrate/dehydrate/verify` run against the in-memory daemon backend; dirty and pinned entries are refused eviction. No real eviction policy, byte budget, or on-disk LRU yet.
4. Writes/conflicts/locks. — **PARTIAL.** `file.write` / `file.read` are spine and round-trip through the read-write FUSE mount (`biohazardfs-fuse mount-workspace`); lock `acquire/release/extend/list/status` are spine; conflict `list/show` are spine. Still **SCAFFOLD**: `file.delete/move/copy/restore`, `conflict.resolve/preserve_all`, `cache.evict/move/repair`, `lock.break`, and FUSE `mkdir/symlink/unlink/rmdir/rename` await promotion (periphery `method_not_implemented`, or `EROFS` / `EIO` at the mount).
5. Electron/shadcn workspace shell. — **SCAFFOLD.** Biohazard Workspace renders Daemon, Workspace, Cache, Transfer, Conflict, Lock, and Onboarding cards from live daemon state; read-only monitoring, no mutating actions yet.
6. Packaging foundation: one installer bundles desktop app, CLI, daemon, and per-user autostart. — **SCAFFOLD.** Electron Builder config and resource staging exist for desktop artifacts; release binaries can be staged into the app package. Still planned: real platform install validation, per-user daemon autostart registration, signing/notarization, and uninstall behavior.
7. Release channels: dev, nightly, alpha, beta, stable. — **SCAFFOLD.** Channels are defined in packaging docs and surfaced in desktop Settings; packaged-only update checks are wired with auto-download disabled. Release publishing/checksum/signing workflows remain planned.
8. Windows native placeholder spike. — **PLANNED.** Windows runs Rust check/test in CI only; no FUSE-on-Windows driver yet.
9. macOS native placeholder spike. — **PLANNED.** macOS runs Rust check/test in CI only; no FUSE-for-macOS integration yet.
10. Kitsu worksets and folder templates. — **PLANNED.** Server `worksets.list` spine exists against the metadata baseline; Kitsu integration has not started.

## LucidLink parity gates

The product spec defines LucidLink parity as a user-visible workflow target, not an implementation clone. These gates are the practical roadmap checkpoints before BiohazardFS can claim production-ready parity:

1. **Mounted workspace parity** — native OS mount on Linux plus at least one artist platform target; authorized namespace appears immediately; ordinary DCC/editor apps can open files from the mount.
2. **Hydration/cache parity** — on-demand hydrate, explicit pin/make-available-offline, remove-local-copy, cache location/limit controls, safe cache-full behavior, and persistent dirty/offline state across daemon restart.
3. **Collaboration parity** — newly added files appear quickly for permitted collaborators with honest upload/availability state; saves upload without manual sync choreography; users do not coordinate versions through side channels.
4. **Version/conflict/lock parity** — every committed write is immutable, divergent edits preserve both sides, conflicts are surfaced in mount + app, and binary/scene-file locks prevent avoidable collisions.
5. **Onboarding parity** — install, authenticate via invite/device/token, mount assigned workspace/workset, and open the linked folder in under 10 minutes for a nontechnical freelancer.
6. **Admin/recovery parity** — admin can revoke devices, manage users/permissions/worksets, inspect audit/version history, recover deleted/overwritten work, and share deep links without exposing storage/database credentials.
7. **Self-hosted parity extension** — all of the above must run against self-hosted PostgreSQL plus S3-compatible object storage, with hosted LucidLink-style convenience but studio-controlled infrastructure.
