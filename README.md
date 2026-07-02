<h1 align="center">BiohazardFS</h1>

**An open-source virtual workspace filesystem for VFX production — built for artists, studios, and AI agents.**

BiohazardFS is a self-hostable file workspace for production teams that need huge project trees, local caching, safe writes, explicit version/audit history, and agent-friendly automation without making artists manage storage plumbing.

> [!IMPORTANT]
> BiohazardFS is under active development. The repository currently contains product contracts, architecture docs, and a Rust scaffold. Do not use it for production data yet.

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
- S3-compatible object storage backend
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

The repo is in planning/scaffolding mode.

Current completed foundations:

- Rust workspace scaffold
- strict cross-platform CI
- product spec
- CLI/agent contract
- local daemon API contract
- metadata schema contract
- filesystem/cache semantics contract
- packaging/release contract
- initial agent skill stubs

Start with:

- [`docs/SPEC.md`](docs/SPEC.md) — product contract
- [`docs/COMMANDS.md`](docs/COMMANDS.md) — CLI and agent contract
- [`docs/DAEMON_API.md`](docs/DAEMON_API.md) — local daemon API
- [`docs/METADATA_SCHEMA.md`](docs/METADATA_SCHEMA.md) — server/control-plane schema
- [`docs/FILESYSTEM_SEMANTICS.md`](docs/FILESYSTEM_SEMANTICS.md) — filesystem/cache safety rules
- [`docs/PACKAGING.md`](docs/PACKAGING.md) — installer and release-channel policy

## Installation

No production installer is published yet.

For local development:

```bash
git clone https://github.com/Biohazard-VFX/BiohazardFS.git
cd BiohazardFS
cargo check --workspace --all-features
cargo test --workspace --all-features
```

Future public artifacts will target:

- macOS `.dmg`
- Windows `.exe`, with `.msi` later
- Linux AppImage, deb, and rpm

## CLI direction

The CLI will default to structured JSON and expose schema introspection for agents.

Planned examples:

```bash
biohazardfs auth status
biohazardfs daemon status
biohazardfs mount status
biohazardfs file history /Project/Shot010/scene.nk
biohazardfs cache pin /Project/Shot010
biohazardfs snapshot list --limit 20
biohazardfs audit events --path /Project/Shot010 --limit 50
biohazardfs schema command file.history
biohazardfs mcp
```

See [`docs/COMMANDS.md`](docs/COMMANDS.md).

## AI agent skill stubs

This repository includes placeholder agent skills so the repo shape is ready for agent-native distribution later.

See the [Skills Index](docs/skills.md):

- [`skills/biohazardfs-shared/SKILL.md`](skills/biohazardfs-shared/SKILL.md)
- [`skills/biohazardfs-workspace/SKILL.md`](skills/biohazardfs-workspace/SKILL.md)
- [`skills/biohazardfs-admin-audit/SKILL.md`](skills/biohazardfs-admin-audit/SKILL.md)

The current skills are intentionally stubs, not authoritative operational instructions. The docs remain the source of truth until the CLI/daemon behavior exists and is tested.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-features
cargo test --workspace --all-features
```

CI runs the full Linux suite plus Windows/macOS check+test. See [`docs/CI.md`](docs/CI.md).

## Contributing

Contributions are welcome, but the safety bar is intentionally high. Read [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`AGENTS.md`](AGENTS.md) before changing behavior.

## Security

Do not report security issues through public issues. See [`SECURITY.md`](SECURITY.md).

## License

Apache-2.0. See [`LICENSE`](LICENSE).
