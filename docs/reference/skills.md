# BiohazardFS Skills Index

BiohazardFS keeps agent skills in this repository so the project can eventually ship agent-native operating guidance alongside the CLI.

Current status: **stub only**. These skills are placeholders and are not authoritative operational instructions yet. Use the docs as the source of truth until the CLI/daemon behavior exists and is tested.

## Skills

| Skill | Description |
|---|---|
| [biohazardfs-shared](../../skills/biohazardfs-shared/SKILL.md) | Stub for future shared CLI patterns, safety rules, mutation policy, output parsing, daemon boundaries. |
| [biohazardfs-workspace](../../skills/biohazardfs-workspace/SKILL.md) | Stub for future workspace operations: status, mount, cache, file history, snapshots, locks, conflicts. |
| [biohazardfs-admin-audit](../../skills/biohazardfs-admin-audit/SKILL.md) | Stub for future admin/audit operations: invites, shares, grants, devices, tokens, audit events, release safety. |

## Installation pattern

Until an installer exists, agents can read these skills directly from the repository.

Future package/install flows should support installing all skills or individual skills from this repo, similar to other agent-skill-enabled CLIs.

## Skill maintenance policy

- Keep skills as stubs until the corresponding CLI/daemon behavior exists and is tested.
- Update skills when CLI commands, safety rules, output envelopes, or auth behavior changes.
- Keep skills concise and operational once they become authoritative.
- Do not include secrets, private endpoints, or private Biohazard-only assumptions.
- Prefer command schemas and docs links over duplicating large reference tables.
- Skills should reinforce safety invariants: dry-run/apply, dehydrate is not delete, dirty data is sacred, conflicts preserve all versions.
