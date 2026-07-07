# Changelog

All notable changes to BiohazardFS will be documented in this file.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) style and uses SemVer-style product versions once releases begin.

## Changelog policy

- Every user-visible change should update this file in the same pull request/commit.
- Group changes under `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, and `Security` where appropriate.
- Include docs-only changes when they alter product contracts, contributor workflow, CI, packaging, or security policy.
- Do not include secrets, private hostnames, private customer/project names, or internal-only incident detail.
- Release entries should include release date, channel, and links to tags/releases once available.
- Breaking CLI/API/schema/filesystem behavior must be called out explicitly.
- Security fixes may use a minimal public note until coordinated disclosure is complete.

## [Unreleased]

### Added

- macOS FUSE support for the `biohazardfs-fuse` adapter when macFUSE is installed and approved, including desktop IPC to launch a daemon-backed `/Volumes/Biohazard` Finder-visible workspace mount with an owner-only local token, per-user cache directory, BiohazardFS volume icon metadata, and daemon-backed directory create/rename support for Finder.
- Dev-loopback daemon state can now persist to an owner-only JSON state file (`BIOHAZARDFS_STATE_PATH`), and the packaged desktop app stores namespace/cache/content scaffold state under its user data directory so Finder-created files and folders survive daemon restart.
- Biohazard Workspace desktop UI rebuilt as a LucidLink-style client: sidebar navigation (Files, Activity, Cache, Conflicts, Settings) with live status badges and a cache-usage footer, topbar with sync-status pill and filter, status bar, and five views with loading/empty/error states. Real shadcn/ui (Radix) primitives replace the hand-rolled scaffold CSS. The raw daemon-diagnostics dump moved into Settings → Diagnostics, out of the default artist flow. All daemon IPC, loopback/token safety, and the renderer's defensive parsing + sync guards (dirty-file dehydration guard, clear-all-local-cache refuse-while-dirty, keep-last-good on dropout, lock default-to-locked) are preserved and now unit-tested. Dark theme by default; accent `#dd4132`.
- Vitest added for the Electron renderer with unit coverage of the safety-critical pure helpers (`isDirtyEntry`, `keepLastGood`, `computeProgress`, `formatBytes`, `extractData`/`extractError`, `entryList`, `stateLabel`).
- Archiv Grotesk wired as the primary UI typeface for the dev/preview build.

### Known follow-ups (not in this change)

- Archiv Grotesk is currently the Trial cut, single weight. It must be replaced with a properly licensed multi-weight cut (or an OFL alternative such as Inter/Geist) before any public/alpha artifact ships.
- Tray/menu-bar residency, close-to-tray, and quit-guard-while-uploading remain a follow-up slice; this change ships the full desktop window only.

- Rust workspace scaffold for core, API types, CLI, daemon, and filesystem adapter crates.
- Runnable `biohazardfs` CLI scaffold with JSON response envelopes and daemon status integration.
- Runnable `biohazardfsd` daemon scaffold with explicit dev-loopback JSON-RPC, local-token auth, loopback-only enforcement, and daemon status/health methods.
- Runnable Biohazard Workspace Electron scaffold using React, TypeScript, Tailwind, and shadcn-compatible design tokens.
- Runnable `biohazardfs-server` scaffold with `serve`, `worker`, `migrate`, `health`, and `version` modes.
- Postgres migration foundation for `biohazardfs-server migrate`, including the MVP metadata baseline tables and `schema_migrations` records.
- TOML-backed database config for `biohazardfs-server migrate` and DB-backed `/readyz`, while keeping database URLs redacted and out of argv.
- First authenticated Postgres-backed namespace read endpoint: `GET /api/v1/namespace/children`, backed by unique token hashes and org-scoped live-node filtering.
- CLI `biohazardfs namespace children` command that calls the authenticated server namespace API using `BIOHAZARDFS_SERVER_TOKEN` from the environment.
- Server-side RustFS/S3-compatible object-store admin commands: `biohazardfs-server object-store check` and `biohazardfs-server object-store ensure-bucket`.
- Live RustFS object-store smoke coverage for signed bucket check/ensure behavior with credential redaction assertions.
- First authenticated server content-object transfer endpoints: `PUT /api/v1/objects/content` and `GET /api/v1/objects/content?sha256=<hash>` backed by RustFS.
- CLI content-object transfer commands: `biohazardfs object put <path>` and `biohazardfs object get --sha256 <hash> --out <path>`.
- First metadata-backed file workflow: `PUT`/`GET /api/v1/files/content` plus `biohazardfs file put/get`, recording file nodes and current versions in Postgres while storing content in RustFS.
- Local daemon workspace runtime methods: `workspace.status` and `workspace.list`, bridged by `biohazardfs daemon workspace-status/list` and smoke-tested through the owner-token loopback daemon.
- Electron workspace visibility now calls daemon workspace status/list through preload IPC and surfaces root/list state in the desktop scaffold smoke.
- Server HTTP scaffold endpoints for `/healthz`, `/readyz`, `/version`, and `/api/v1/status`.
- Server API scaffold reference documentation.
- Linux client smoke script that verifies daemon, CLI, and Electron launch together over authenticated dev-loopback JSON-RPC.
- Concrete repository static-analysis script for Rust, Electron, shell scripts, GitHub Actions, and whitespace checks.
- Server smoke script, Docker image build gate, and dev Compose config validation.
- Product contract docs for BiohazardFS.
- Agent-first CLI contract.
- Local daemon API contract.
- Server/control-plane metadata schema contract.
- Filesystem and cache semantics contract.
- CI and release-gate policy.
- Packaging and release-channel contract.
- Strict cross-platform CI with Linux full suite, Electron typecheck/ESLint/Prettier/build, ShellCheck, actionlint, Hadolint, client smoke, server smoke, Docker/Compose validation, and Windows/macOS check+test.
- Cargo dependency/license/security audit policy.
- Initial stub agent skills directory.
- Public-facing README draft.
- Security policy and contributing guide.
- Spec bulk-completion, Layer 0: typed domain model in `biohazardfs-core` (node/version/lock/conflict/operation/event/snapshot/grant/org records, prefixed-ID validators, relative-path normalization + case-insensitive sibling uniqueness, cache state machine enforcing dirty/pinned-never-evicted), plus `biohazardfs-api-types` event envelope, mutation classification + dry-run `OperationToken` model, and the `known_methods` registry (single source of truth for daemon/server/CLI method names).
- Spec bulk-completion, daemon: `DaemonBackend` with an in-memory mock backing the cache/lock/conflict/transfer/operation-token/audit state; every method in the daemon registry is wired, with the read/cache/lock/conflict/config spine implemented against the in-memory backend and periphery mutations returning typed `method_not_implemented`; `agent-safe` mutation profile with dry-run operation tokens; event-stream buffering (`daemon.events.subscribe`).
- Spec bulk-completion, server: `003_metadata_baseline` migration adding `locks`, `conflicts`, `snapshots`, `grants`, `shares`, `publishes`, `invites`, `projects`, `worksets`, `workset_rules`, `devices`, `trash_records`, and `retention_policies`; spine routes against Postgres for locks/conflicts/operations/trash/audit/devices/projects/worksets (with scope enforcement and idempotent operation replay); `admin` server mode; response DTOs in `biohazardfs-api-types`.
- Spec bulk-completion, CLI: the seven global flags (`--output`, `--fields`, `--cursor`, `--source`, `--request-id`, `--dry-run`, `--yes`); the full command tree across every `COMMANDS.md` namespace; `agent-safe` mutation gating with dry-run/apply (exit codes 5 conflict/lock, 7 confirmation, 8 unsupported-platform); schema registry sourced from `known_methods`; `biohazardfs mcp serve` stdio stub.
- Spec bulk-completion, desktop: Biohazard Workspace cards for cache manager, transfer queue, conflict panel, lock state, offline/degraded banner, and an onboarding skeleton, each over the existing daemon loopback IPC with loading/empty/error states and artist-facing language.
- Spec bulk-completion, FUSE: read-write `WorkspaceFs` mount (`biohazardfs-fuse mount-workspace`) alongside the read-only mount, with hydrate-on-open into a local cache dir, per-handle write buffering, and `create`/`write`/`flush`/`fsync`/`release` routing through the daemon. Dirty data is never silently lost: a flush reports success only after `daemon file.write` acknowledges an immutable `FileVersion`; on RPC failure the buffer is restored for retry and the cache entry marked dirty, with EIO surfaced to the artist. Daemon `file.write` and `file.read` promoted to spine (record `FileVersion` + `Operation` + `AuditEvent`, transition cache through the core state machine, enforce case-insensitive sibling uniqueness and optimistic-concurrency version checks). Live `/dev/fuse` write-then-read smoke coverage. `rename`/`unlink`/`mkdir`/`symlink` are scaffold-boundary (EROFS/EIO) pending daemon `file.move`/`file.delete` and an operation-token RPC.

### Changed

- `scripts/ci/fuse-smoke.sh` now runs on Linux and macOS, skipping safely when the platform FUSE runtime is unavailable or macFUSE still needs Privacy & Security approval.
- Biohazard Workspace now uses app-owned frameless chrome by default on macOS, with traffic-light window controls drawn inside the UI.
- CLI: the `object get` and `file get` local-file flag is renamed from `--output <path>` to `--out <path>`. `--output <json|ndjson|text>` is now the global output-format flag, matching `docs/reference/COMMANDS.md`.
- Daemon `dispatch_rpc` is now stateful: `dispatch_rpc(backend: &DaemonBackend, request: &DaemonRequest)`. `DevLoopbackConfig` carries an `Arc<DaemonBackend>`; construct via `DevLoopbackConfig::new(addr, token)` or `DevLoopbackConfig::with_backend(addr, token, backend)`. `DaemonHttpClient` and `call::<T>` are unchanged.
- `scripts/ci/server-db-smoke.sh` now asserts migration count 3 and validates all 23 metadata tables.
- README and `AGENTS.md` now reflect the current CLI/server command surface, smoke scripts, Docker/Compose/Helm validation, and CI gates.
- Repository documentation now treats docs as product contracts for implementation work.

### Fixed

- Biohazard Workspace first-run onboarding can now be skipped or completed even while the local workspace is still unconfigured, and the Back button behavior is covered by a renderer test.
- FUSE read-write partial-write corruption: non-truncating edits to existing files committed a sparse zero-padded buffer instead of overlaying the existing content (repro: `abcdef` + seek 3 + write `X` became `00 00 58`). The write buffer is now seeded from the hydrated cache file on first write, and flush/close are a no-op when no writes occurred.
- FUSE dirty-data loss on release: after a failed daemon flush, `release` dropped the only in-memory copy of the dirty bytes. Unsynced bytes are now persisted to a durable on-disk dirty journal (`<cache_dir>/dirty/`) on flush failure and release, and cleared on successful commit.
- Security: `biohazardfs-fuse mount-workspace --local-token` and `biohazardfs auth login --token` exposed secrets in argv (process listings / shell history). Both flags are removed; the local daemon token is read from `BIOHAZARDFS_LOCAL_TOKEN` and the login token from `BIOHAZARDFS_TOKEN` (env only).
- Windows CI: a migration-003 SQL assertion was line-ending sensitive and failed under CRLF checkouts. The SQL is now `\r`-normalized before the multi-line substring check.
- Daemon path resolution: CLI file/cache/lock commands sent `{path}` but the daemon required `node_id`, returning `missing_param node_id`. The daemon now resolves either `node_id` or a mount-relative `path` (case-insensitive walk from the namespace root) for the spine lookup methods.
- Server `operations.submit` idempotency race: two concurrent submissions with the same idempotency key could both miss the pre-read, with the loser mapped to `operation_store_unavailable` instead of replaying. The insert now uses `ON CONFLICT (org_id, idempotency_key) DO NOTHING` and replays the existing operation on the no-row path.
- CLI mutation gate honesty: `--yes` on daemon-gated destructive/admin/data-moving methods no longer dispatches to the daemon (which would reject with `operation_token_required`); it returns a typed `apply_not_wired` (exit 7) explaining that daemon-issued operation tokens are not yet wired. `--apply <operation-token>` is documented as planned.
- FUSE preexisting files read as empty: mounted files backed by existing daemon content advertised size 0 at lookup, so `stat`/`dd`/Python saw empty content. `file.list`/`file.stat` now carry `size_bytes` and the mount advertises the real size at lookup/getattr.
- FUSE zero-byte create/truncate durability: `: > mount/empty.txt` no longer disappears. `create` and `O_TRUNC` now seed the write buffer with an empty vec so flush commits a zero-byte version even when no write syscall follows.
- FUSE acknowledged-write durability: buffered writes are persisted to the on-disk dirty journal on each `write()` (best-effort, survives process death), so acknowledged bytes are not lost if the FUSE process dies before flush.
- FUSE stale-handle detection: writes now capture `base_version_id` at open and forward it in `file.write`, so the daemon's `version_conflict` check catches stale concurrent handles instead of last-writer-wins.
- Daemon content-leak in provenance: `file.write` no longer stores `content_hex` (the file bytes) in `Operation.params_json` or `AuditEvent.payload_json`. Provenance now carries only safe metadata (node/parent/name/mode, version_id, content_hash, size_bytes).
- Daemon operation-token binding: token validation now binds method/classification/source in addition to the params hash, so a token issued for one operation cannot be replayed against a different method/classification/source.
- CLI dry-run hint accuracy: the dry-run response no longer tells users to "re-run with --yes to apply"; it states that apply is not wired (`apply_not_wired`) and daemon-issued tokens / `--apply` are planned.
- Server idempotency payload check: `operations.submit` with an existing `idempotency_key` but a differing payload now returns `operation_idempotency_payload_mismatch` instead of silently replaying the prior operation.
- Server migration checksum line-ending independence: `checksum_sql` strips `\r` before hashing so LF and CRLF checkouts produce identical checksums (already-applied dev DBs need a fresh migrate; smoke uses ephemeral containers).
- Docs sweep: README and `docs/reference/COMMANDS.md` no longer show removed `--local-token`/`auth login --token` examples, and `--yes`/`--apply` wording reflects the env-only token policy and `apply_not_wired` behavior.
- FUSE truncate-to-zero (and general truncation) is now durable: `setattr(size)` truncates the cache file, stages the surviving bytes in the handle's write buffer so the next flush commits a real version, and updates the inode size. `: > mount/existing.txt` and Python `open(path, "wb")` now commit a zero-byte version instead of silently keeping the old content. Sparse extension past the current length returns `ENOSYS` (documented gap). Covered by a live `fuse-smoke` assertion.
- FUSE same-handle re-flush: a successful flush now advances every open handle's `base_version_id` to the daemon-returned version, so `write → flush → write` on one handle no longer self-conflicts.
- Daemon `cache.hydrate` now rejects `Dirty` entries (`entry_dirty`) instead of overwriting unsynced local data.
- Daemon lock semantics: `lock.acquire` rejects an existing effective lock on the same target (`lock_conflict`); `lock.release` checks ownership (`lock_not_owner` on mismatch).
- Server `operations.submit` idempotency replay now compares the full semantic request (`kind`, `source`, `node_id`, `base_version_id`, `device_id`, `params`), not just `params`, so a same-key resubmission with any differing field returns `operation_idempotency_payload_mismatch`.

[Unreleased]: https://github.com/Biohazard-VFX/BiohazardFS/compare/main...HEAD
