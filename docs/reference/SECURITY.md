# BiohazardFS Security Model

Status: draft reference
Audience: contributors, daemon/server implementers, packaging maintainers, security reviewers

This document describes the security expectations for BiohazardFS. For vulnerability reporting, see the root [`SECURITY.md`](../../SECURITY.md).

## Security goals

- Keep artist/client data private and scoped to authorized users, devices, worksets, shares, and projects.
- Prevent normal artist clients from receiving permanent storage/database credentials.
- Keep local daemon control limited to the owning OS user plus an owner-only local session token.
- Make destructive/admin/data-moving operations explicit, auditable, and guarded.
- Preserve data under conflict/offline/cache-full conditions.
- Redact secrets from CLI output, logs, audit events, support bundles, and error messages.

## Credential model

Normal clients should use:

- invite/device enrollment
- revocable device sessions
- scoped API tokens where needed
- short-lived transfer authorization

Normal clients should not use:

- permanent object-storage credentials
- direct database credentials
- broad admin tokens for routine file access
- unredacted signed URLs in logs or support bundles

Token rules:

- Store token hashes server-side where possible.
- Store local secrets in OS keyring when available.
- Use owner-only fallback files only for dev/headless contexts.
- Redact all secrets by default.

## Local daemon security

`biohazardfsd` local API security is defined in [`DAEMON_API.md`](../architecture/DAEMON_API.md).

Required controls:

- same OS user boundary
- owner-only local session token
- owner-only endpoint descriptor and token files
- no public network binding by default
- optional loopback HTTP only for dev/test/integration mode
- no server credential exposure through local daemon status APIs

## Filesystem safety and data-loss security

Data loss is a security issue for this project.

Required invariants:

- Delete in the mount means server/cloud trash.
- Dehydrate/remove-local-copy never deletes server data.
- Dirty/unuploaded files are never auto-evicted.
- Pinned files are not auto-evicted.
- Divergent work creates conflicts and preserves all versions.
- Server versions commit only after content integrity verification.
- Cache-full behavior must fail safely before data loss.
- Uninstall/update must not remove cache/config/user data unless the user explicitly selects purge/remove-data.

## Path, symlink, and mount boundaries

Required controls:

- Normalize and validate paths before authorization.
- Reject traversal outside authorized roots.
- Reject control characters and dangerous path ambiguity.
- Constrain symlinks to authorized roots unless explicit policy allows otherwise.
- Enforce case-insensitive sibling uniqueness by default for cross-platform safety.

## Audit and provenance

Meaningful operations should record:

- actor
- impersonated user, if any
- device
- source: UI, CLI, agent, API, server, or test
- request ID
- operation ID
- affected node/path/version/share/snapshot IDs
- result
- timestamp

Audit payloads must not contain secrets.

## Agent-specific security

Agents are fast and confident, not trusted.

Agent-facing commands must:

- default to JSON envelopes
- validate JSON payloads strictly
- expose schema introspection
- require dry-run/apply tokens for destructive/admin/data-moving operations when policy requires
- include request IDs and stable error codes
- avoid hidden prompts in noninteractive mode

## Release and supply-chain security

Release rules are defined in [`CI.md`](CI.md) and [`PACKAGING.md`](PACKAGING.md).

Before stable/public-grade releases:

- macOS artifacts should be signed and notarized
- Windows artifacts should be signed
- Linux artifacts should provide checksums and package/repository signatures where practical
- release artifacts should include checksums, version, channel, commit SHA, and build metadata
- SBOM/provenance should be added when the release process matures

## Threats to test

Security and safety tests should cover:

- local daemon token bypass attempts
- path traversal and symlink escape
- unauthorized node/workset/share access
- revoked device/token access
- secret redaction in CLI/logs/support bundles
- cache-full and dirty-data preservation
- conflict preservation under offline divergence
- installer/update/uninstall data-preservation behavior
