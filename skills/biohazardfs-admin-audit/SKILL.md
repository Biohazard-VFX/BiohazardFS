---
name: biohazardfs-admin-audit
description: "Stub skill for future BiohazardFS admin and audit operations. Not yet authoritative."
metadata:
  version: 0.0.0-stub
  status: stub
  openclaw:
    category: "filesystem"
    requires:
      bins:
        - biohazardfs
    skills:
      - biohazardfs-shared
---

# BiohazardFS Admin/Audit Skill Stub

This is a placeholder for the future BiohazardFS admin and audit skill.

It is intentionally not authoritative yet. Use the repository docs as source of truth:

- `docs/COMMANDS.md`
- `docs/METADATA_SCHEMA.md`
- `docs/SECURITY.md`
- `docs/CI.md`
- `docs/PACKAGING.md`

Future versions of this skill should cover:

- audit event queries
- invite/share/grant workflows
- device and token revocation
- support bundle rules
- release safety checks
- admin mutation guardrails
