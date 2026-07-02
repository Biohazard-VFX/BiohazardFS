---
name: biohazardfs-shared
description: "Stub skill for future BiohazardFS shared CLI and safety rules. Not yet authoritative."
metadata:
  version: 0.0.0-stub
  status: stub
  openclaw:
    category: "filesystem"
    requires:
      bins:
        - biohazardfs
---

# BiohazardFS Shared Skill Stub

This is a placeholder for the future shared BiohazardFS agent skill.

It is intentionally not authoritative yet. Use the repository docs as source of truth:

- `docs/product/SPEC.md`
- `docs/reference/COMMANDS.md`
- `docs/architecture/DAEMON_API.md`
- `docs/architecture/FILESYSTEM_SEMANTICS.md`
- `docs/reference/SECURITY.md`

Future versions of this skill should cover:

- CLI JSON envelope handling
- schema introspection
- dry-run/apply safety
- secret redaction
- daemon vs direct-server boundaries
- core safety invariants
