# BiohazardFS Repository Guidelines

- Repo: https://github.com/Biohazard-VFX/BiohazardFS
- Product: BiohazardFS, an open-source virtual sync filesystem for VFX.
- Desktop app: Biohazard Workspace.
- Primary implementation language: Rust.
- Desktop shell: Electron + React + TypeScript + Tailwind + shadcn/ui.
- License target: Apache-2.0.
- Canonical product spec: `docs/product/SPEC.md`.
- Canonical architecture doc: `docs/architecture/ARCHITECTURE.md`.
- Canonical command surface: `docs/reference/COMMANDS.md`.

This project has a deliberately high quality bar. BiohazardFS is filesystem and sync software: sloppy code can corrupt work, lose artist time, or make recovery impossible five days before delivery. Prefer boring, obvious, testable code over clever abstractions every time.

## Core Engineering Philosophy

- Make data structures obvious.
- Make control flow obvious.
- Make failure states explicit.
- Keep interfaces small.
- Do not hide expensive work behind innocent-looking calls.
- Do not paper over bugs with retries, sleeps, or broad catch-alls.
- Do not add abstraction because it feels architecturally elegant; add abstraction only after concrete duplication proves it is needed.
- Optimize for the maintainer reading this code at 3 a.m. while a production artist cannot open a shot.

Code should be self-describing. Comments explain why, invariants, tradeoffs, and external quirks. Comments must not narrate obvious syntax.

## Required Workflow

1. Read `docs/product/SPEC.md` before changing product behavior.
2. Read `docs/architecture/ARCHITECTURE.md` before changing daemon, filesystem, cache, transfer, or server boundaries.
3. Keep docs aligned with behavior changes:
   - `README.md`
   - `docs/product/SPEC.md`
   - `docs/product/ROADMAP.md`
   - `docs/architecture/ARCHITECTURE.md`
   - `docs/architecture/SERVER_ARCHITECTURE.md`
   - `docs/architecture/SERVER_API.md`
   - `docs/architecture/DAEMON_API.md`
   - `docs/architecture/METADATA_SCHEMA.md`
   - `docs/architecture/FILESYSTEM_SEMANTICS.md`
   - `docs/reference/COMMANDS.md`
   - `docs/reference/CONFIG.md`
   - `docs/reference/SECURITY.md`
   - `docs/reference/CI.md`
   - `docs/reference/PACKAGING.md`
   - `docs/reference/SMOKE.md`
4. Make changes in the smallest coherent vertical slice.
5. Add tests with behavior changes. If a change cannot be tested yet, document why and add the seam that will make it testable.
6. Run validation before committing.
7. Commit only scoped, related changes. Do not sweep unrelated local work into your commit.

## Project Structure

Expected repository shape:

```text
crates/
  core/                 # domain logic: models, policy, cache state, transfer state
  api-types/            # shared API request/response/event/schema types
  cli/                  # thin CLI over core/daemon/server APIs; JSON-first
  daemon/               # local daemon: mounts, transfer queue, cache manager
  fuse/                 # Linux FUSE adapter/prototype
  server/               # public server/control-plane binary and modules
apps/
  workspace-electron/   # Electron shell, React UI, shadcn components
  admin-web/            # reserved for future admin web UI
  docs-site/            # reserved for future docs site
deploy/
  docker/               # Docker packaging
  helm/                 # Helm charts
  compose/              # local/dev compose stacks
packaging/              # macOS, Windows, Linux installer/release assets
docs/
  product/              # product spec and roadmap
  architecture/         # architecture and behavioral contracts
  reference/            # CLI/config/CI/security/packaging/smoke references
  adr/                  # architecture decision records
  operations/           # self-hosting and operations runbooks
generated/              # generated schemas, MCP manifests, CLI reference
tests/                  # product-level smoke/integration/filesystem safety tests
scripts/                # CI/dev/release helper scripts
skills/                 # repository-provided agent skills
```

Crate directory names should stay concise and unprefixed inside `crates/`; the package names may still use `biohazardfs-*` for published crate identity.

Architectural rule: Electron is a UI shell. Rust owns filesystem semantics, cache state, transfer state, auth/session state, conflict handling, and all safety-critical behavior.

## Build, Test, and Static Analysis

### Rust baseline

Before committing Rust changes, run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo check --workspace --all-features
git diff --check          # unstaged working-tree whitespace
git diff --cached --check # staged whitespace before commit
```

Use `cargo fmt --all` to fix formatting.

CI must run equivalent gates. A pull request is not reviewable if it does not pass formatting, clippy with `-D warnings`, tests, and committed-diff whitespace checks.

### TypeScript/Electron baseline

`apps/workspace-electron` uses pnpm. Every UI change must pass the package scripts defined there:

```bash
pnpm --dir apps/workspace-electron install --frozen-lockfile
pnpm --dir apps/workspace-electron run static
pnpm --dir apps/workspace-electron run build
```

For full local static analysis, run:

```bash
scripts/ci/static-analysis.sh
```

For Linux client integration changes, also run:

```bash
scripts/ci/client-smoke.sh
```

For server/control-plane changes, also run:

```bash
scripts/ci/server-smoke.sh
docker build -f deploy/docker/server/Dockerfile -t biohazardfs-server:local .
docker compose -f deploy/compose/dev/docker-compose.yml config --quiet
```

Do not use npm for this app. If the project ever changes package manager, update this file and CI in the same change. Do not leave undocumented local build requirements.

### Non-Rust static analysis

CI also enforces:

- Electron ESLint with strict TypeScript rules and React hooks rules.
- Electron Prettier formatting checks.
- ShellCheck for shell scripts.
- actionlint for GitHub Actions workflows.
- Hadolint for Dockerfiles.
- Helm lint/template for the server chart.
- Server smoke tests for health, readiness, version, status, and mode commands.
- Docker server image build and Compose config validation.

### Full repo validation target

The long-term validation target is one command that runs all required checks:

```bash
just ci
```

Until `just` tasks exist, run the explicit commands above.

## Rust Code Conventions

### General style

- Use Rust 2024 edition.
- Treat warnings as errors.
- Prefer explicit types at module boundaries and public APIs.
- Prefer small modules with clear ownership over large utility dumping grounds.
- Prefer enums with data over stringly-typed state.
- Prefer `Result<T, E>` with domain errors over `anyhow` in library crates.
- `anyhow` is acceptable in binaries/tests where context matters more than typed recovery.
- Use `thiserror` for library error enums.
- Do not use `unwrap()` or `expect()` in production code except for true invariants that cannot fail. Add a short comment explaining the invariant.
- Do not use `panic!` for recoverable runtime errors.
- Do not use `todo!()`/`unimplemented!()` in committed code paths. Tests may use them only when clearly unreachable.
- Avoid `unsafe`. Any `unsafe` requires a documented safety invariant and reviewer attention.
- Avoid macros unless they remove real repetition without hiding control flow.
- Avoid global mutable state. Prefer explicit handles/context objects.
- Do not spawn background tasks without a shutdown path.
- Do not block async runtimes with filesystem/network calls; use appropriate blocking boundaries.

### API and data design

- Make invalid states unrepresentable where reasonable.
- Use IDs/newtypes for file IDs, version IDs, device IDs, user IDs, workset IDs, and transfer IDs.
- Do not pass raw strings for paths across core boundaries when a typed path/newtype would prevent mistakes.
- Keep serialization types in `biohazardfs-api-types` separate from internal mutable state when the distinction matters.
- Version public wire formats from the start.
- All timestamps crossing APIs should be explicit UTC values.
- All byte sizes are bytes unless the type name says otherwise.

### Filesystem/sync safety

Filesystem and sync code must be defensive:

- Dirty/unuploaded data must never be auto-evicted.
- Conflicts must preserve all versions.
- Cloud delete and local dehydration are different operations.
- Rename/write/flush/fsync semantics must be tested before claiming DCC compatibility.
- Never silently downgrade a failed upload to “synced.”
- Never silently discard local state after daemon restart.
- If state recovery is uncertain, enter safe mode and require explicit user/admin action.

### Logging and errors

- Logs are for debugging; CLI stdout JSON is for machines. Do not mix them.
- Logs must never include secrets, bearer tokens, signed URLs, raw credentials, or private file contents.
- Errors should include actionable context: operation, path/id, recoverability, and next step where possible.
- Do not expose implementation-only error text to artists if a human-friendly error can be produced at the boundary.

## TypeScript, Electron, and shadcn Code Conventions

### TypeScript

- Use strict TypeScript.
- No `any` unless a boundary absolutely requires it; isolate and validate unknown input immediately.
- Prefer discriminated unions for UI state.
- Do not use `// @ts-ignore` or `// @ts-nocheck` without explicit maintainer approval.
- Keep renderer code deterministic and side-effect-light.
- Keep daemon interaction behind typed client modules.
- Never duplicate daemon state logic in the renderer. The UI renders daemon state; it does not invent sync truth.

### React/Electron

- Electron main process owns app lifecycle, tray/menu, installer/update hooks, and daemon launch/connection.
- Renderer process owns presentation.
- No filesystem/sync decision logic in React components.
- Components should be small and boring. Extract state machines and API clients out of components.
- Avoid long prop chains. Use focused context/providers only where they simplify real shared state.
- Do not add heavy UI dependencies when shadcn/Radix/Tailwind primitives are enough.

### shadcn/ui and visual design

- Use shadcn/ui primitives as the default component vocabulary.
- Keep the UI minimal, calm, and readable.
- Use semantic status colors sparingly and consistently.
- Artist-facing text must avoid backend jargon: say “Make available offline,” “Remove local copy,” “Syncing,” “Conflict,” not “hydrate object chunk.”
- Advanced diagnostics can use technical language but must be tucked away from the default artist flow.
- Empty/loading/error states are required for every async UI surface.

## CLI and Agent-Native Conventions

BiohazardFS is CLI-native and agent-native from day one.

Use a CLI contract similar to mature agent-facing CLIs, but keep the requirements explicit in this repo:

- JSON output by default for machine-oriented commands.
- Stable JSON error envelope.
- `commands schema --format json` for discoverability.
- Redacted `smoke run --format json` for validation.
- `config path`, `config show --redacted`, and `config doctor` style commands.
- OS keyring credential backend with owner-only local fallback for dev/headless.
- Dry-run and `--yes` guardrails for destructive operations.
- Secret-redacted JSONL audit/provenance logs.
- Document the exact command contract in `docs/reference/COMMANDS.md`; do not require contributors to inspect another local repository to understand expected behavior.

Agents may administer the system only when authorized. If an agent acts on behalf of a user, provenance must record the actor, target user, entry point, command/API operation, and time.

## Test-Driven Development Requirements

Every behavior change needs tests at the lowest useful layer.

Expected test categories:

- Unit tests for pure core logic.
- State-machine tests for cache, transfer, conflict, lock, and safe-mode behavior.
- Serialization compatibility tests for API types.
- CLI snapshot/contract tests for JSON output and error envelopes.
- Filesystem adapter tests for open/read/write/rename/delete/fsync semantics.
- Integration tests for daemon restart/recovery.
- DCC behavior fixtures for Nuke, Houdini, Blender, Unreal, Resolve, and Premiere workflows as they are discovered.

Do not rely only on happy-path tests. Every safety invariant needs a failing-case test.

Critical invariants to test:

- Dirty files are not evicted.
- Dehydrate never deletes cloud data.
- Cloud delete goes to trash/recycler.
- Conflicts preserve all versions.
- Interrupted uploads resume or fail safely.
- Daemon restart does not lose dirty transfer state.
- Placeholder read triggers hydration.
- Cached reads do not hit the network.
- Unauthorized paths are hidden or denied according to policy.

## CI Requirements

CI must be boring and strict.

Minimum CI jobs:

- Linux full suite:
  - Rust format check.
  - Rust clippy with `-D warnings`.
  - Rust test workspace.
  - Rust check workspace all features.
  - `git diff --check` equivalent for whitespace.
  - Rust dependency/security/license audit.
- Windows check/test.
- macOS check/test.
- TypeScript install/typecheck/lint/test/build once Electron app is real.

No release artifacts should be cut when required CI fails, required smoke tests for claimed features/platforms fail or are missing, or known critical data-loss/security blockers are open.

See `docs/reference/CI.md` for CI and release-gate policy.

## Security Rules

- Never commit credentials, tokens, API keys, signed URLs, storage secrets, personal project data, or private artist/client files.
- Do not print secrets in test output, debug logs, audit logs, CLI JSON, or UI error panes.
- Permanent storage/database credentials must not be the normal artist-client auth model.
- Use short-lived tokens or server-mediated transfer authorization for normal clients.
- Device sessions must be revocable.
- Auth and transfer scopes must be least-privilege.
- Public share links must support expiry and revocation.
- Any security-sensitive behavior change must update `docs/reference/SECURITY.md`.

## Build and Packaging Requirements

BiohazardFS must be installable by normal artists and scriptable by technical users.

Distribution targets:

- Native desktop installers/downloads.
- Homebrew.
- npm wrapper for CLI/install convenience.
- Website download link.
- Versioned release artifacts.

Packaging must account for:

- one platform-native installer per OS as the primary distribution path
- Rust daemon/CLI binaries bundled with the desktop app installer
- per-user daemon autostart registration
- platform filesystem/placeholder driver or runtime prerequisites
- release channels: `dev`, `nightly`, `alpha`, `beta`, `stable`
- one product version across desktop app, CLI, daemon, and server/control-plane artifacts at first
- auto-update path for desktop app and Rust binaries
- admin privilege requirements
- clean uninstall that preserves user cache/config unless explicit purge/remove-data is requested

Packaging code must be tested. Installer scripts are production code. See `docs/reference/PACKAGING.md`.

## Documentation Requirements

Docs are part of the product.

When behavior changes, update relevant docs in the same change.

Required docs:

- `README.md`: product intro and quick start.
- `CONTRIBUTING.md`: contributor workflow and expectations.
- `CHANGELOG.md`: user-visible, contract, CI, packaging, and security changes.
- `SECURITY.md`: vulnerability reporting policy.
- `docs/product/SPEC.md`: product contract.
- `docs/reference/COMMANDS.md`: command surface.
- `docs/architecture/SERVER_ARCHITECTURE.md`: server/control-plane runtime contract.
- `docs/architecture/SERVER_API.md`: current server API scaffold contract.
- `docs/architecture/DAEMON_API.md`: local daemon API contract.
- `docs/architecture/METADATA_SCHEMA.md`: server/control-plane metadata contract.
- `docs/architecture/FILESYSTEM_SEMANTICS.md`: mounted filesystem and cache behavior.
- `docs/architecture/ARCHITECTURE.md`: system design and boundaries.
- `docs/reference/CONFIG.md`: config, profiles, env vars, credential storage.
- `docs/reference/SECURITY.md`: threat model and security behavior.
- `docs/reference/CI.md`: CI and release-gate policy.
- `docs/reference/PACKAGING.md`: packaging, installers, release channels, and artifact policy.
- `docs/reference/SMOKE.md`: validation workflows.
- `docs/product/ROADMAP.md`: planned phases and non-goals.
- `docs/reference/skills.md`: index for repository-provided agent skills.
- `docs/adr/0001-repository-layout.md`: accepted repository layout decision.
- `AGENTS.md`: this file.

Docs must use placeholders for hostnames, users, paths, and credentials unless a value is intentionally public.

Changelog policy:

- Update `CHANGELOG.md` for user-visible behavior, product contracts, CI/release gates, packaging, security, contributor workflow, and public-facing docs changes.
- Do not include secrets, private hostnames, private customer/project names, or uncoordinated security details.

Agent skills policy:

- Keep `skills/` entries as stubs until the corresponding CLI/daemon behavior exists and is tested.
- When skills become authoritative, update them in the same change as relevant CLI or behavior changes.

## Dependency Policy

- Dependencies must earn their weight.
- Prefer well-maintained, boring dependencies with clear licenses.
- Avoid framework churn.
- Do not add dependencies for tiny helpers.
- Do not add native dependencies casually; every native dependency affects installation and support.
- Pin or lock dependency versions through normal lockfiles.
- Security-sensitive or filesystem-adjacent dependency changes require extra scrutiny.

## Git and Commit Conventions

- Keep commits focused and reviewable.
- Do not mix formatting churn with semantic changes unless the formatting is caused by the touched lines.
- Do not commit unrelated local work.
- Do not rewrite history on shared branches without explicit approval.
- Commit messages should be concise and action-oriented.
- Prefer conventional-ish prefixes when useful: `docs:`, `feat:`, `fix:`, `test:`, `refactor:`, `chore:`.

Before pushing, ensure:

```bash
git status --short
```

Only intended changes should be staged.

## Review Bar

A change is not acceptable if it:

- hides complex behavior behind vague abstractions;
- weakens a safety invariant;
- lacks tests for new behavior;
- produces warnings;
- makes errors less actionable;
- leaks secrets;
- duplicates source of truth between daemon and UI;
- adds unexplained global state;
- breaks JSON command contracts;
- changes on-disk/wire formats without versioning;
- makes filesystem behavior surprising to DCC apps.

## Agent-Specific Notes

- Treat this repository as safety-critical infrastructure, not a prototype toy.
- Read the spec first; update it when product truth changes.
- If a requirement is ambiguous, ask or write an ADR before building around assumptions.
- Prefer small, validated vertical slices.
- Keep unrelated dirty files untouched.
- End handoffs with exact commands run and what remains risky or unvalidated.
