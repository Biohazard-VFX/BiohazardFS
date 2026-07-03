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

### Changed

- Repository documentation now treats docs as product contracts for implementation work.

[Unreleased]: https://github.com/Biohazard-VFX/BiohazardFS/compare/main...HEAD
