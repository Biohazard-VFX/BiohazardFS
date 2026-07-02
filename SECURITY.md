# Security Policy

BiohazardFS is filesystem, sync, storage, and access-control software. Please treat security and data-loss issues as sensitive even while the project is pre-1.0.

## Reporting a vulnerability

Please do **not** report security vulnerabilities through public GitHub issues.

Use GitHub private vulnerability reporting if it is enabled for this repository. If private reporting is not available, contact the maintainers through a private channel and include only the minimum detail needed to establish contact. Do not post exploit details publicly before coordination.

## What to report

Please report issues involving:

- unauthorized file, project, workset, share, or snapshot access
- token/session/device revocation failures
- local daemon authentication bypass
- privilege escalation through installer, daemon, helper, or filesystem integration
- symlink/path traversal or mount escape
- secret leakage in logs, CLI JSON, support bundles, errors, or audit events
- signed URL or storage credential exposure
- data loss, silent overwrite, or conflict-preservation failure
- cache/dehydrate behavior that deletes or corrupts server data
- unsafe uninstall/update behavior that deletes cache/config/user data without explicit consent
- supply-chain, release, or update-channel compromise

## Safe handling expectations

When reporting, include:

- affected version/commit
- operating system and install channel, if relevant
- exact command or workflow, with secrets redacted
- request IDs, operation IDs, audit event IDs, or logs if available
- whether files were dirty, pinned, offline, locked, conflicted, or being uploaded

Do not include private production files unless maintainers explicitly request a minimized reproduction.

## Project security invariants

BiohazardFS should maintain these invariants:

- normal clients do not receive permanent storage/database credentials
- local daemon access requires same OS user boundary plus local session token
- secrets are redacted from CLI output, logs, audit events, and support bundles
- device sessions are revocable
- server delete is separate from local cache removal
- dirty/unuploaded files are never auto-evicted
- divergent work is preserved as conflicts, not silently overwritten
- admin/destructive/data-moving operations are auditable and guarded

## Supported versions

BiohazardFS has not reached a stable release. Security fixes currently target the `main` branch until release branches exist.
