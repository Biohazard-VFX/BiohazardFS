# Contributing to BiohazardFS

Thanks for your interest in BiohazardFS.

BiohazardFS is filesystem and sync software. The contribution bar is intentionally high because mistakes can lose artist work, leak private data, or make recovery impossible near delivery.

## Project status

BiohazardFS is pre-production. The repo currently contains architecture/product contracts and an early Rust scaffold. Expect breaking changes before v1.0.

Start by reading:

- [`README.md`](README.md)
- [`AGENTS.md`](AGENTS.md)
- [`docs/product/SPEC.md`](docs/product/SPEC.md)
- [`docs/reference/COMMANDS.md`](docs/reference/COMMANDS.md)
- [`docs/architecture/FILESYSTEM_SEMANTICS.md`](docs/architecture/FILESYSTEM_SEMANTICS.md)
- [`docs/reference/CI.md`](docs/reference/CI.md)
- [`docs/reference/SECURITY.md`](docs/reference/SECURITY.md)

## Development setup

```bash
git clone https://github.com/Biohazard-VFX/BiohazardFS.git
cd BiohazardFS
cargo check --workspace --all-features
cargo test --workspace --all-features
```

Recommended local checks before pushing:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo check --workspace --all-features
cargo test --workspace --all-features
git diff --check
```

CI also runs Windows/macOS check+test and Rust dependency/security/license policy.

## Contribution rules

- Keep changes focused and reviewable.
- Do not mix unrelated refactors, formatting churn, and behavior changes.
- Update relevant docs when behavior changes.
- Update [`CHANGELOG.md`](CHANGELOG.md) for user-visible, contract, CI, security, packaging, or contributor-facing changes.
- Add tests for new behavior.
- Do not weaken safety invariants without explicit design discussion.
- Do not commit secrets, private production files, signed URLs, tokens, or credentials.

## Safety invariants

These rules are not optional:

- Delete is not dehydrate.
- Dirty/unuploaded data must not be auto-evicted.
- Divergent work must not be silently overwritten.
- Conflicts preserve all versions.
- Server-visible file versions require content integrity verification.
- Local daemon auth must preserve the same-user + local-token boundary.
- Secrets must be redacted from CLI output, logs, audit events, and support bundles.

## Pull request expectations

A good PR includes:

- a clear summary
- linked issue or rationale
- tests or explanation of why tests do not apply
- docs updates if behavior/contracts changed
- changelog entry when applicable
- risk notes for filesystem, sync, security, packaging, or migration changes

## Commit messages

Use concise, action-oriented messages. Conventional-style prefixes are welcome:

```text
docs: define cache semantics
ci: add cross-platform checks
feat: add schema registry skeleton
fix: preserve dirty state on daemon restart
```

## Agent skills policy

The `skills/` directory currently contains stubs only. Do not turn them into authoritative operational skills until the corresponding CLI/daemon behavior exists and is tested.

When skills become real:

- keep them concise
- make safety invariants explicit
- prefer schema introspection over duplicated command tables
- update skills in the same change as CLI behavior changes

## Security issues

Do not report vulnerabilities in public issues. See [`SECURITY.md`](SECURITY.md).

## Releases

Release policy is defined in [`docs/reference/PACKAGING.md`](docs/reference/PACKAGING.md) and [`docs/reference/CI.md`](docs/reference/CI.md). Do not publish release artifacts when CI/smoke gates fail or critical data-loss/security blockers are open.
