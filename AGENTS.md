---
Repo: Biohazard-VFX/BiohazardFS
Primary language: Rust
Desktop shell: Electron + React + shadcn/ui
Canonical spec: docs/SPEC.md
Canonical agent file: AGENTS.md
Status: planning/scaffold
---

BiohazardFS is an open-source LucidLink-style virtual sync filesystem for VFX.

## Required workflow

1. Read `docs/SPEC.md` before implementing product behavior.
2. Keep `README.md`, `docs/SPEC.md`, `docs/COMMANDS.md`, and `docs/ARCHITECTURE.md` aligned with user-facing behavior changes.
3. Keep the Rust core/daemon/CLI authoritative; Electron is a UI shell, not the sync engine.
4. All agent-facing CLI commands must emit stable JSON by default.
5. Destructive or cloud-mutating commands must support `--dry-run` and require explicit `--yes` when appropriate.
6. Never print, log, commit, or repeat storage credentials, API tokens, refresh tokens, signed URLs, or private file contents.
7. Prefer thin vertical slices with tests over broad speculative implementation.

## Validation expectations

When code exists, maintain these gates:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
git diff --check
```

Electron validation will be added once `apps/workspace-electron` is implemented.

## Product invariants

- Artists must not need to understand S3, Postgres, FUSE, or raw backend credentials.
- Files appear virtually before bytes are local.
- Cache is user-configurable and safe to clear/dehydrate for synced files.
- Dirty/unuploaded data must never be auto-evicted.
- Conflicts preserve every version; never silently overwrite.
- Kitsu can be a source of worksets/assignments, but BiohazardFS must also work without Kitsu.
- CLI and agents are first-class users.
