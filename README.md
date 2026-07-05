<h1 align="center">BiohazardFS</h1>

**An open-source virtual workspace filesystem for VFX production — built for artists, studios, and AI agents.**

BiohazardFS is a self-hostable file workspace for production teams that need huge project trees, local caching, safe writes, explicit version/audit history, and agent-friendly automation without making artists manage storage plumbing.

> [!IMPORTANT]
> BiohazardFS is under active development. The repository currently contains product contracts, architecture docs, runnable Rust service/client scaffolds, an Electron workspace shell, and CI smoke paths. Do not use it for production data yet.

## What it is

BiohazardFS aims to provide one installed desktop workspace where authorized files appear immediately, hydrate into local cache on open or pin, sync writes safely, preserve conflicts, expose explicit file history, and let agents operate the same system through a deterministic JSON CLI.

The target first-run experience:

1. Download the installer for your OS.
2. Install Biohazard Workspace.
3. Paste an invite/device code or token.
4. Choose a cache location.
5. Get a mounted workspace with the projects you are allowed to see.

## Why BiohazardFS?

VFX teams need a filesystem that understands production realities:

- very large files and directories
- image sequences
- DCC apps with temp/autosave/atomic-write behavior
- binary scene files that need locks, not merges
- freelancers and vendors with scoped access
- local cache/pin/dehydrate workflows
- point-in-time recovery
- clear audit history for humans and agents

BiohazardFS is designed around these rules:

- **Never silently overwrite divergent work.** Conflicts preserve all versions.
- **Delete is not dehydrate.** Deleting in the mount means server trash; removing local cache is a separate action.
- **Dirty data is sacred.** Dirty/unuploaded files are never auto-evicted.
- **Agents are first-class users.** The CLI is JSON-first, schema-introspectable, and built with safety rails.
- **One installer should just work.** The desktop installer should bundle the app, CLI, daemon, and per-user autostart.

## Planned architecture

- Rust core, daemon, CLI, and filesystem adapters
- Electron desktop app: Biohazard Workspace
- React + TypeScript + Tailwind + shadcn/ui frontend
- RustFS-first S3-compatible object storage backend
- PostgreSQL control/metadata database
- Optional project-tracker integrations for assignments/worksets
- Agent-native JSON CLI from day one

```text
Biohazard Workspace desktop app
  ↓ local daemon API
biohazardfs CLI / agent tools
  ↓
biohazardfsd per-user daemon
  ↓
filesystem adapter + local cache + SQLite local state
  ↓
BiohazardFS control plane
  ↓
S3-compatible object storage + PostgreSQL metadata
```

## Current status

The repo is still pre-release scaffold: in-memory daemon state, deferred local SQLite, Linux-first FUSE, and no published installers. The runnable foundation now spans the full CLI command tree, the daemon method registry, the server control-plane spine, a Linux read-write FUSE mount, and the Electron workspace cards.

Three honest status levels apply throughout the codebase:

- **IMPLEMENTED** — real behavior, exercised by tests.
- **SCAFFOLD** — typed, wired, and exercised through a tested seam, but backed by an in-memory/mock backend or returning `method_not_implemented` / `operation_not_implemented`.
- **PLANNED** — in the spec, not yet in code.

Current completed foundations:

- Rust workspace scaffold for core, shared API types, CLI, daemon, FUSE adapter, and server/control-plane crates
- Layer 0 domain model in `biohazardfs-api-types` / `biohazardfs-core`: the canonical method/operation/command registry (`known_methods`), mutation classification, and the event envelope shared by the daemon, server, and CLI so the three surfaces cannot drift
- runnable `biohazardfs` CLI (IMPLEMENTED) with the full command tree — mount, file, cache, transfer, snapshot, lock, conflict, workset, invite, share, grant, publish, audit, admin, auth, schema, mcp, plus status/version/doctor/smoke — JSON/ndjson/text output, field-mask/pagination/provenance globals, schema introspection (`schema list`, `schema command`, `commands`), and an agent-safe mutation gate (`--dry-run` mints an operation token, `--yes` applies)
- runnable `biohazardfsd` daemon (SCAFFOLD) over dev-loopback JSON-RPC-like HTTP with local-token auth; the full method registry is wired, with read/low-risk spine methods running against an in-memory backend and destructive/admin/data-moving periphery methods returning `method_not_implemented` after the operation-token check
- runnable `biohazardfs-fuse` Linux adapter: a read-only source mount (`mount`, IMPLEMENTED) and a read-write workspace mount (`mount-workspace`, SCAFFOLD) that hydrates files on open via `file.read` and commits writes on flush/fsync via `file.write`, with dirty/pinned data never silently lost
- runnable Electron + React + TypeScript + Tailwind Biohazard Workspace shell (SCAFFOLD) rendering live daemon state through Daemon, Workspace, Cache, Transfer, Conflict, Lock, and Onboarding cards; read-only monitoring, no mutating actions yet
- runnable `biohazardfs-server` foundation with health/readiness/version/status endpoints, worker/config commands, Postgres migrations (including the metadata baseline for devices, projects, worksets, locks, conflicts, snapshots, grants, shares, publishes, invites, trash, and retention), RustFS/S3-compatible object-store checks, content/file transfer APIs, and DB-backed spine routes for locks, conflicts, operations, trash, audit, devices, projects, and worksets; periphery routes return `operation_not_implemented`
- Linux smoke paths covering the daemon/CLI/Electron client loop, server API and Postgres migrations, RustFS bucket setup, end-to-end object/file transfers, and both FUSE mounts (read-only and read-write)
- Docker server image, dev Compose stack, and Helm chart scaffolds
- strict CI gates: the Linux job runs Rust, Electron, shell, workflow, Dockerfile, Docker/Compose, Helm, and smoke checks; Windows and macOS run Rust check/test gates
- product spec, CLI/agent contract, local daemon API contract, metadata schema contract, filesystem/cache semantics contract, packaging/release contract, and initial agent skill stubs

Start with:

- [`docs/product/SPEC.md`](docs/product/SPEC.md) — product contract
- [`docs/reference/COMMANDS.md`](docs/reference/COMMANDS.md) — CLI and agent contract
- [`docs/architecture/DAEMON_API.md`](docs/architecture/DAEMON_API.md) — local daemon API
- [`docs/architecture/SERVER_ARCHITECTURE.md`](docs/architecture/SERVER_ARCHITECTURE.md) — server/control-plane runtime
- [`docs/architecture/SERVER_API.md`](docs/architecture/SERVER_API.md) — current server API scaffold
- [`docs/architecture/METADATA_SCHEMA.md`](docs/architecture/METADATA_SCHEMA.md) — server/control-plane schema
- [`docs/architecture/FILESYSTEM_SEMANTICS.md`](docs/architecture/FILESYSTEM_SEMANTICS.md) — filesystem/cache safety rules
- [`docs/reference/CONFIG.md`](docs/reference/CONFIG.md) — shared typed runtime config contract
- [`docs/reference/PACKAGING.md`](docs/reference/PACKAGING.md) — installer and release-channel policy
- [`docs/reference/CI.md`](docs/reference/CI.md) — CI and release-gate policy
- [`docs/reference/SMOKE.md`](docs/reference/SMOKE.md) — smoke-validation policy and workflow direction

## Installation

No production installer is published yet.

For local development, install Rust 1.91 or newer, Node 22, pnpm 10.33, and Docker if you want to run server/storage smoke tests. Then run:

```bash
git clone https://github.com/Biohazard-VFX/BiohazardFS.git
cd BiohazardFS
cargo check --workspace --all-features
cargo test --workspace --all-features
pnpm --dir apps/workspace-electron install --frozen-lockfile
pnpm --dir apps/workspace-electron run static
pnpm --dir apps/workspace-electron run build
```

To install and run the current Linux client scaffold locally:

```bash
cargo install --path crates/cli --force
cargo install --path crates/daemon --force
export BIOHAZARDFS_LOCAL_TOKEN=local_dev_token
biohazardfsd --dev-loopback-http --addr 127.0.0.1:47666
```

In another terminal:

```bash
export BIOHAZARDFS_LOCAL_TOKEN=local_dev_token
biohazardfs daemon status
pnpm --dir apps/workspace-electron exec electron dist/electron/main.js
```

For the automated Linux smoke paths used by CI:

```bash
scripts/ci/client-smoke.sh
scripts/ci/server-smoke.sh
scripts/ci/server-db-smoke.sh
scripts/ci/object-store-smoke.sh
scripts/ci/server-transfer-smoke.sh
scripts/ci/fuse-smoke.sh
```

Future public artifacts will target:

- macOS `.dmg`
- Windows `.exe`, with `.msi` later
- Linux AppImage, deb, and rpm

## CLI direction

The CLI defaults to structured JSON for implemented scaffold commands and calls the daemon through the same JSON-RPC-like method envelope described in `docs/architecture/DAEMON_API.md`. The current loopback HTTP transport is development/test-only; production transport is still intended to be platform IPC discovered from an owner-only descriptor.

Currently runnable (representative; JSON by default):

```bash
biohazardfs status
biohazardfs daemon status
biohazardfs daemon methods
biohazardfs daemon workspace-status
biohazardfs daemon workspace-list --path plates
biohazardfs config path
biohazardfs config show --redacted
biohazardfs config validate
biohazardfs mount status
biohazardfs cache status
biohazardfs lock list
biohazardfs audit events --limit 50
biohazardfs snapshot list --limit 20
biohazardfs auth status
biohazardfs schema list
biohazardfs schema command daemon.status
biohazardfs commands
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs namespace children
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs object put ./file.bin
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs object get --sha256 <hash> --out ./file.bin
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs file put ./shot001.exr --name shot001.exr
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs file get --node <node-id> --out ./shot001.exr
```

`--output <json|ndjson|text>` is the global response-format flag. `object get` and `file get` write a downloaded blob to a local file with `--out <path>` (renamed from the earlier `--output`); the CLI refuses to overwrite an existing path.

The wider command tree (transfer, conflict, workset, invite, share, grant, publish, admin, auth credentials, schema event/error/config/all, mcp serve) parses and dispatches against the same envelope. Read and low-risk commands run against the in-memory daemon backend and return honest state (often an empty list, e.g. `snapshot list` or `conflict list`, until state is seeded); destructive, admin, and data-moving commands require `--dry-run` or `--yes`, and the daemon methods they ultimately target still return `method_not_implemented` until their backing is promoted.

Server/control-plane scaffold commands:

```bash
biohazardfs-server serve --addr 127.0.0.1:8080
biohazardfs-server health
biohazardfs-server version
biohazardfs-server config
biohazardfs-server worker
BIOHAZARDFS_DATABASE_URL=<postgres-url> biohazardfs-server migrate
biohazardfs-server --config ./config.toml --profile dev object-store check
biohazardfs-server --config ./config.toml --profile dev object-store ensure-bucket
```

Planned command surface still includes the server-mediated apply flow for operation tokens (today the CLI mints them), the `config doctor` diagnostic, and a redacted `smoke run --format json`. The stdio MCP surface (`biohazardfs mcp serve`) is already wired for `initialize`, `ping`, `tools/list`, and `tools/call` against the same command registry.

## FUSE mounts (Linux)

`biohazardfs-fuse` ships two mounts, both Linux-only and foreground-only:

- `mount --source <dir> --mountpoint <dir>` — read-only virtual view of an existing workspace/source tree (IMPLEMENTED; verified by live-mount smoke).
- `mount-workspace --daemon-endpoint <host:port> --local-token <token> --cache-dir <dir> --mountpoint <dir>` — read-write BiohazardFS workspace backed by the local daemon (SCAFFOLD). Files hydrate on open via `file.read`; writes buffer per file handle and commit one blob per flush/fsync via `file.write`; dirty and pinned data is never silently lost.

The read-write mount has real gaps today: `mkdir`, `symlink`, `unlink`, `rmdir`, and `rename` return `EROFS`/`EIO` because the daemon-side `file.delete`/`file.move` and directory-create methods are still periphery, and the FUSE layer cannot yet mint the operation tokens those destructive/data-moving methods require. See [`docs/architecture/FILESYSTEM_SEMANTICS.md`](docs/architecture/FILESYSTEM_SEMANTICS.md).

See [`docs/reference/COMMANDS.md`](docs/reference/COMMANDS.md).

## AI agent skill stubs

This repository includes placeholder agent skills so the repo shape is ready for agent-native distribution later.

See the [Skills Index](docs/reference/skills.md):

- [`skills/biohazardfs-shared/SKILL.md`](skills/biohazardfs-shared/SKILL.md)
- [`skills/biohazardfs-workspace/SKILL.md`](skills/biohazardfs-workspace/SKILL.md)
- [`skills/biohazardfs-admin-audit/SKILL.md`](skills/biohazardfs-admin-audit/SKILL.md)

The current skills are intentionally stubs, not authoritative operational instructions. The docs remain the source of truth until the CLI/daemon behavior exists and is tested.

## Development

```bash
scripts/ci/static-analysis.sh
scripts/ci/client-smoke.sh
scripts/ci/server-smoke.sh
scripts/ci/server-db-smoke.sh
scripts/ci/object-store-smoke.sh
scripts/ci/server-transfer-smoke.sh
scripts/ci/fuse-smoke.sh
```

The dev Compose scaffold uses Postgres plus RustFS, matching BiohazardFS's self-hosted storage direction:

```bash
docker build -f deploy/docker/server/Dockerfile -t biohazardfs-server:local .
docker compose -f deploy/compose/dev/docker-compose.yml config --quiet
helm lint deploy/helm/biohazardfs --set secrets.existingSecret=biohazardfs-secret
helm template biohazardfs deploy/helm/biohazardfs --set secrets.existingSecret=biohazardfs-secret >/tmp/biohazardfs-helm.yaml
```

CI runs the full Linux suite, Electron build/smoke, server smoke, Postgres DB smoke, RustFS object-store smoke, server transfer smoke, Docker/Compose/Helm validation, and Windows/macOS check+test. See [`docs/reference/CI.md`](docs/reference/CI.md).

## Contributing

Contributions are welcome, but the safety bar is intentionally high. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`AGENTS.md`](AGENTS.md) before changing behavior.

## Security

Do not report security issues through public issues. See [`SECURITY.md`](SECURITY.md).

## License

Apache-2.0. See [`LICENSE`](LICENSE).
