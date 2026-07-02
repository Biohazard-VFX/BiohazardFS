---
name: biohazardfs-workspace
description: "Stub skill for future BiohazardFS workspace operations. Not yet authoritative."
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

# BiohazardFS Workspace Skill Stub

This is a placeholder for the future BiohazardFS workspace operations skill.

It is intentionally not authoritative yet. Use the repository docs as source of truth:

- `docs/reference/COMMANDS.md`
- `docs/architecture/DAEMON_API.md`
- `docs/architecture/FILESYSTEM_SEMANTICS.md`
- `docs/reference/SMOKE.md`

Future versions of this skill should cover:

- mount/status workflows
- cache, pin, hydrate, and dehydrate workflows
- file history/version restore workflows
- snapshot workflows
- lock and conflict workflows
- offline queue/reconciliation workflows
