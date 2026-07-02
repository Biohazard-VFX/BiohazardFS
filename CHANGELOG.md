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
- Product contract docs for BiohazardFS.
- Agent-first CLI contract.
- Local daemon API contract.
- Server/control-plane metadata schema contract.
- Filesystem and cache semantics contract.
- CI and release-gate policy.
- Packaging and release-channel contract.
- Strict cross-platform CI with Linux full suite and Windows/macOS check+test.
- Cargo dependency/license/security audit policy.
- Initial stub agent skills directory.
- Public-facing README draft.
- Security policy and contributing guide.

### Changed

- Repository documentation now treats docs as product contracts for implementation work.

[Unreleased]: https://github.com/Biohazard-VFX/BiohazardFS/compare/main...HEAD
