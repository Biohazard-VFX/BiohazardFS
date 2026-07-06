# BiohazardFS Roadmap Draft

Status legend: **IMPLEMENTED** (real behavior, tested), **SCAFFOLD** (typed + wired + tested seam, in-memory/mock backend or returns `method_not_implemented` / `operation_not_implemented`), **PLANNED** (in the spec, not yet in code).

1. ADRs and skeleton. — **IMPLEMENTED.** ADR `0001-repository-layout` is accepted; the Rust workspace (core, api-types, cli, daemon, fuse, server), the Layer 0 method/operation/command registry shared across all three surfaces, the Electron shell, Docker/Compose/Helm scaffolds, and CI gates are in place.
2. Linux read-only FUSE prototype. — **IMPLEMENTED.** `biohazardfs-fuse mount` exposes a source workspace tree read-only via `ReadOnlyWorkspaceFs`, verified by a live-mount smoke (`scripts/ci/fuse-smoke.sh`) that checks read and rejects write.
3. Hydration/cache prototype. — **SCAFFOLD.** Cache state machine plus `cache.status/list/pin/unpin/hydrate/dehydrate/verify` run against the in-memory daemon backend; dirty and pinned entries are refused eviction. No real eviction policy, byte budget, or on-disk LRU yet.
4. Writes/conflicts/locks. — **PARTIAL.** `file.write` / `file.read` are spine and round-trip through the read-write FUSE mount (`biohazardfs-fuse mount-workspace`); lock `acquire/release/extend/list/status` are spine; conflict `list/show` are spine. Still **SCAFFOLD**: `file.delete/move/copy/restore`, `conflict.resolve/preserve_all`, `cache.evict/move/repair`, `lock.break`, and FUSE `mkdir/symlink/unlink/rmdir/rename` await promotion (periphery `method_not_implemented`, or `EROFS` / `EIO` at the mount).
5. Electron/shadcn workspace shell. — **SCAFFOLD.** Biohazard Workspace renders Daemon, Workspace, Cache, Transfer, Conflict, Lock, and Onboarding cards from live daemon state; read-only monitoring, no mutating actions yet.
6. Packaging foundation: one installer bundles desktop app, CLI, daemon, and per-user autostart. — **PLANNED.** No native installers; `cargo install` and `pnpm` dev runs are the only paths today.
7. Release channels: dev, nightly, alpha, beta, stable. — **PLANNED.**
8. Windows native placeholder spike. — **PLANNED.** Windows runs Rust check/test in CI only; no FUSE-on-Windows driver yet.
9. macOS native placeholder spike. — **PLANNED.** macOS runs Rust check/test in CI only; no FUSE-for-macOS integration yet.
10. Kitsu worksets and folder templates. — **PLANNED.** Server `worksets.list` spine exists against the metadata baseline; Kitsu integration has not started.
