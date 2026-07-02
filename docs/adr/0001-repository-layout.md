# ADR 0001: Repository layout

Status: accepted
Date: 2026-07-02

## Context

BiohazardFS is growing beyond a Rust client scaffold. The product includes:

- Rust client core, daemon, CLI, filesystem adapters, and server/control plane
- Electron desktop app
- public self-hosted server
- Docker and Helm deployment assets
- platform installers
- generated schemas and MCP manifests
- agent skills
- product, architecture, reference, operations, and ADR documentation
- smoke, integration, and filesystem safety tests

A flat `docs/` directory and a small undifferentiated workspace will not scale cleanly.

## Decision

Use a purpose-based monorepo layout:

```text
crates/                 Rust workspace crates
apps/                   desktop/admin/docs applications
deploy/                 Docker, Helm, compose, and deployment assets
packaging/              macOS, Windows, Linux installer/release assets
docs/product/           product spec and roadmap
docs/architecture/      architecture and behavioral contracts
docs/reference/         CLI/config/CI/security/packaging/smoke references
docs/adr/               architecture decision records
docs/operations/        self-hosting and operational runbooks
generated/              generated schemas, MCP manifests, CLI reference
tests/                  product-level smoke/integration/filesystem safety tests
scripts/                CI/dev/release helper scripts
skills/                 repository-provided agent skills
```

The server/control plane lives in the public repo from the start and has an in-repo Dockerfile and Helm chart skeleton.

## Consequences

- Contributors and agents can find the correct contract by category.
- Deployment and packaging scripts have stable homes instead of accumulating in `.github/` or app directories.
- Product-level tests have a place separate from crate-local unit tests.
- Generated artifacts have a stable directory for future CI stale-generation checks.
- Docs links must be kept aligned after moves.

## Follow-ups

- Add implementation to `crates/server` behind the server architecture contract.
- Expand `deploy/helm/biohazardfs` once server config and health endpoints exist.
- Add real packaging scripts under `packaging/` once desktop bundling starts.
- Add product-level smoke and filesystem-safety tests under `tests/`.
