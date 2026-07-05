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

### Changed

- CLI: the `object get` and `file get` local-file flag is renamed from `--output <path>` to `--out <path>`. `--output <json|ndjson|text>` is now the global output-format flag, matching `docs/reference/COMMANDS.md`.
- Daemon `dispatch_rpc` is now stateful: `dispatch_rpc(backend: &DaemonBackend, request: &DaemonRequest)`. `DevLoopbackConfig` carries an `Arc<DaemonBackend>`; construct via `DevLoopbackConfig::new(addr, token)` or `DevLoopbackConfig::with_backend(addr, token, backend)`. `DaemonHttpClient` and `call::<T>` are unchanged.
- `scripts/ci/server-db-smoke.sh` now asserts migration count 3 and validates all 23 metadata tables.
- README and `AGENTS.md` now reflect the current CLI/server command surface, smoke scripts, Docker/Compose/Helm validation, and CI gates.
- Repository documentation now treats docs as product contracts for implementation work.

[Unreleased]: https://github.com/Biohazard-VFX/BiohazardFS/compare/main...HEAD
