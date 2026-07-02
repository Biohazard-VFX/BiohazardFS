# BiohazardFS CI and Release Gates

Status: draft policy
Audience: contributors, maintainers, release owners, automation agents

BiohazardFS is filesystem and sync software. CI must catch correctness drift early because a small regression can corrupt work, lose artist time, or make recovery impossible near delivery.

## CI posture

CI is strict from day one.

Required baseline:

- warnings fail
- formatting is enforced
- tests are blocking
- dependency/security/license checks are blocking for Rust
- cross-platform check/test runs on Linux, Windows, and macOS
- generated artifacts must be current once generators exist
- contract snapshots become blocking as soon as the corresponding command/API/schema exists

Docs are reviewed manually. CI does not fail solely because a canonical docs cross-reference is missing.

## Current GitHub Actions matrix

### Linux full suite

Linux is the full quality gate and must run:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-features
cargo test --workspace --all-features
git diff-tree --check --root -r HEAD
cargo deny check advisories bans licenses sources
helm lint deploy/helm/biohazardfs --set secrets.existingSecret=biohazardfs-secret
helm template biohazardfs deploy/helm/biohazardfs --set secrets.existingSecret=biohazardfs-secret
```

### Windows/macOS suite

Windows and macOS run compile/test gates now:

```bash
cargo check --workspace --all-features
cargo test --workspace --all-features
```

This catches cross-platform compile and test failures early without making every static/documentation gate run three times.

## Dependency, license, and security policy

Rust dependency/security/license audits run now.

Current tool:

```bash
cargo deny check advisories bans licenses sources
```

Policy:

- known security advisories fail CI unless explicitly ignored with rationale
- yanked dependencies fail CI
- unknown registries fail CI
- unknown git sources fail CI
- licenses must be compatible with the repository's Apache-2.0 distribution goal
- duplicate dependency versions are warnings unless they become installation/security problems

Electron/npm dependency and license jobs are required once the Electron app has real dependencies and build scripts.

## Contract tests

As soon as a contract surface exists, CI must enforce it.

Blocking contract tests should cover:

- standard JSON response envelope
- CLI command schema snapshots
- daemon method schema snapshots
- daemon event schema snapshots
- error-code schema snapshots
- config schema snapshots
- MCP tool registry snapshots
- generated API/types artifacts, once generators exist

Intentional contract changes must update snapshots in the same pull request/commit.

## Filesystem safety tests

Before any write-support claim, CI must include mock/in-memory safety tests for:

- dirty data survives daemon restart
- dehydrate refuses dirty files
- dehydrate never deletes server/cloud data
- delete creates trash/soft-delete, not local-only dehydrate
- cache-full path pauses downloads and safely blocks/fails new writes
- pinned files are not auto-evicted
- dirty/unuploaded files are not auto-evicted
- case-insensitive sibling collisions are rejected
- lock conflicts block writes or preserve work as conflict copies
- divergent offline reconnect creates conflict records
- no silent overwrite of divergent file content

Before claiming support on a real filesystem/platform, CI or release validation must include platform smoke for that claimed platform.

Examples:

- Linux FUSE claim requires Linux mount/read/write/dehydrate/conflict smoke.
- Windows placeholder/mount claim requires Windows platform smoke.
- macOS placeholder/mount claim requires macOS platform smoke.

## Release artifact gates

Release artifacts must not be cut if any of the following are true:

- required CI is failing
- required smoke tests for claimed features/platforms are missing or failing
- a known critical data-loss blocker is open
- a known critical security blocker is open

Release artifacts include:

- platform-native desktop installers that bundle the desktop app, CLI, daemon, autostart registration, and required platform helpers
- CLI/daemon binaries when published as advanced/standalone artifacts
- package-manager artifacts
- server/control-plane images or bundles

Packaging and release-channel policy is defined in `docs/reference/PACKAGING.md`.

## Warnings and flaky tests

Warnings fail.

Flaky tests may only be quarantined when:

- there is a tracked issue
- the quarantine has an owner or next action
- the test is not covering a data-loss safety invariant

Data-loss safety tests cannot be quarantined away to make a release green.

## Generated artifacts

Once generators exist, CI must fail if generated artifacts are stale.

Expected future generated artifacts may include:

- CLI command schema JSON
- daemon method/event/error schemas
- API type bindings
- MCP tool manifests
- generated CLI reference docs

Required pattern:

```bash
# example future gate
cargo run -p biohazardfs-cli -- schema all > generated/schemas/commands.json
git diff --exit-code
```

## Manual docs review

Docs are part of the product, but docs are reviewed manually rather than enforced through brittle existence/cross-reference checks.

Reviewers should verify relevant docs when behavior changes:

- `docs/product/SPEC.md`
- `docs/product/ROADMAP.md`
- `docs/architecture/ARCHITECTURE.md`
- `docs/architecture/SERVER_ARCHITECTURE.md`
- `docs/architecture/DAEMON_API.md`
- `docs/architecture/METADATA_SCHEMA.md`
- `docs/architecture/FILESYSTEM_SEMANTICS.md`
- `docs/reference/COMMANDS.md`
- `docs/reference/CONFIG.md`
- `docs/reference/SECURITY.md`
- `docs/reference/CI.md`
- `docs/reference/PACKAGING.md`
- `docs/reference/SMOKE.md`
- `docs/reference/skills.md`
- `docs/adr/0001-repository-layout.md`

## First implementation target

The current scaffold CI should establish:

1. Linux full Rust suite.
2. Windows/macOS check+test.
3. Rust dependency/security/license audit.
4. Whitespace check.
5. Helm chart lint/template check.

The next implementation phase should add:

1. CLI envelope tests.
2. CLI schema snapshot tests.
3. Config parse/validation tests.
4. Mock daemon API contract tests.
5. Mock filesystem/cache safety tests.
