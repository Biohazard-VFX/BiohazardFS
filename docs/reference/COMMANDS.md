# BiohazardFS CLI and Agent Contract

Status: draft reference
Audience: implementers, agent authors, automation authors, operators

The `biohazardfs` CLI is a primary product surface, not a debug wrapper. Humans, desktop UI code, CI, render workers, and AI agents should all be able to use the same command contract safely.

## Design principles

1. **JSON by default.** Every command writes a stable JSON response envelope unless explicitly asked for another format.
2. **Canonical JSON inputs.** Complex or mutating commands accept canonical `--json` request payloads. Human-friendly flags are aliases over the same request schema.
3. **Runtime introspection.** Agents can ask the CLI for command, config, event, and error schemas at runtime.
4. **Safe first run.** Fresh installs default to agent-safe mutation behavior until the user chooses a profile.
5. **Traceability.** Every response includes a request ID, actor/device/source metadata when known, and a schema version.
6. **Context discipline.** Large reads have safe limits, truncation metadata, pagination cursors, and optional field masks.
7. **The agent is not trusted.** Validate inputs as adversarial: paths, IDs, control characters, double-encoded values, shell-expanded strings, and ambiguous destructive requests.

## Global defaults

```text
Output default: JSON
Config format: TOML
Config path: platform config dir, e.g. ~/.config/biohazardfs/config.toml on Linux
Credential sources: env token, credential file, device enrollment code
Command style: canonical namespaces with short aliases for common workflows
MCP surface: biohazardfs mcp over stdio
```

Global flags:

```bash
biohazardfs <command> \
  --profile <name> \
  --config <path> \
  --output json|ndjson|text \
  --fields <field-mask> \
  --limit <n> \
  --cursor <cursor> \
  --source cli|agent|ui|api|server \
  --request-id <id> \
  --no-color
```

`--output text` is for humans only. Tests, agents, scripts, and CI should use default JSON or explicit `--output json` / `--output ndjson`.

## Standard response envelope

Every command returns the same top-level envelope shape.

```json
{
  "ok": true,
  "command": "file.history",
  "data": {},
  "warnings": [],
  "error": null,
  "meta": {
    "request_id": "req_01J...",
    "timestamp": "2026-07-02T18:30:00Z",
    "actor": {
      "id": "usr_...",
      "display_name": "Nicholai",
      "impersonated_user_id": null
    },
    "device": {
      "id": "dev_...",
      "name": "workstation"
    },
    "source": "cli",
    "schema_version": "2026-07-commands-v1"
  }
}
```

Error responses still use the same envelope:

```json
{
  "ok": false,
  "command": "file.delete",
  "data": null,
  "warnings": [],
  "error": {
    "code": "confirmation_required",
    "message": "This command requires --dry-run or --yes under the current mutation policy.",
    "details": {
      "policy": "agent-safe",
      "required": ["--dry-run", "--yes"]
    }
  },
  "meta": {
    "request_id": "req_01J...",
    "timestamp": "2026-07-02T18:30:00Z",
    "actor": null,
    "device": null,
    "source": "agent",
    "schema_version": "2026-07-commands-v1"
  }
}
```

Command-specific fields live under `data`. Operation metadata such as pagination, dry-run plan IDs, audit event IDs, and timing may live under `data` or `meta` as defined by the command schema.

## Input model

### Canonical JSON payloads

Every complex or mutating command must accept a canonical request payload:

```bash
biohazardfs snapshot create --json '{
  "scope": {"kind": "workset", "id": "wrk_hero_shot"},
  "name": "before-client-review",
  "retention": "30d"
}'
```

Payloads may also come from files or stdin:

```bash
biohazardfs share create --json @share-request.json
biohazardfs workset create --json - < workset.json
```

Convenience flags are allowed, but they must compile into the same request schema:

```bash
biohazardfs snapshot create --workset wrk_hero_shot --name before-client-review --retention 30d
```

### Params for read commands

Read commands may support `--params` for filters, field masks, and pagination:

```bash
biohazardfs audit events --params '{
  "path": "/Project/Shot010",
  "event_types": ["file.write", "file.restore"],
  "limit": 50,
  "fields": "events(id,type,path,actor,timestamp),next_cursor"
}'
```

### Input hardening

The CLI must reject:

- path traversal outside configured mount/cache roots
- control characters in paths, IDs, names, and comments
- embedded query strings/fragments in resource IDs
- double-encoded path segments where a raw ID/path is expected
- destructive operations with ambiguous paths such as root or empty string
- mixed path and ID targets when the command schema requires exactly one
- unknown JSON fields unless the schema explicitly allows extension fields

Validation failures return `ok: false` with `error.code = "invalid_input"` or a more specific schema-defined code.

## Mutation safety profiles

Fresh installs use the `agent-safe` profile until first-run setup chooses a profile.

First-run setup should ask the user to choose one of:

```text
agent-safe      Safer for agents, CI, automation, and unattended use.
human-friendly  Lower friction for trusted interactive human use.
```

The selected profile is stored in the TOML config and is inspectable:

```bash
biohazardfs config get mutation_policy
```

### Agent-safe profile

Agent-safe profile rules:

- Mutations must use `--dry-run`, `--yes`, or schema-specific apply flow.
- Destructive/admin/data-moving operations require a dry-run operation token before execution.
- Read commands never require confirmation.
- Cache-local operations may be allowed with `--yes` when they cannot delete cloud/server data.

Dry-run operation token flow:

```bash
biohazardfs file delete --json '{"path":"/Project/bad.mov","mode":"trash"}' --dry-run
```

Response data includes:

```json
{
  "dry_run": true,
  "operation_token": "op_01J...",
  "plan_hash": "sha256:...",
  "impact": {
    "files_affected": 1,
    "bytes_affected": 1048576,
    "server_data_removed": false
  }
}
```

Execution must reference the dry-run token:

```bash
biohazardfs file delete --apply op_01J...
```

### Human-friendly profile

Human-friendly profile rules:

- Low-risk mutations may execute directly.
- Destructive/admin/data-moving operations still require `--yes` or a dry-run/apply token.
- Interactive builds may prompt on a TTY, but noninteractive mode must never block waiting for input.

### Stricter operation categories

These operation classes require stricter safety in agent-safe mode:

- permanent delete
- purge/trash empty
- restore/promote version
- bulk move/rename
- conflict resolution that selects/discards a version
- snapshot rollback
- device or token revocation
- share/permission widening
- retention policy changes
- admin/operator mutations

## Output shaping for large reads

Large read commands must use safe defaults:

- default limit
- minimal default fields
- `warnings` when output is truncated
- `data.next_cursor` when more results exist
- `data.truncated = true` when the CLI withheld data
- optional `--fields` field mask
- optional `--output ndjson` for stream processing

Example:

```bash
biohazardfs audit events --path /Project --limit 25
```

```json
{
  "ok": true,
  "command": "audit.events",
  "data": {
    "events": [],
    "next_cursor": "cur_...",
    "truncated": true
  },
  "warnings": [
    {
      "code": "result_truncated",
      "message": "More events are available. Pass --cursor to continue or --limit to adjust page size."
    }
  ],
  "error": null,
  "meta": {"request_id":"req_...","timestamp":"2026-07-02T18:30:00Z","actor":null,"device":null,"source":"cli","schema_version":"2026-07-commands-v1"}
}
```

## Auth and credential commands

Credential lookup precedence:

1. `BIOHAZARDFS_TOKEN`
2. explicit `--credential-file <path>`
3. configured credential file
4. active device enrollment/session

Commands:

```bash
biohazardfs auth status
biohazardfs auth enroll --code <device-or-invite-code>
biohazardfs auth login --token <token>
biohazardfs auth logout --yes
biohazardfs auth whoami
biohazardfs auth credentials path
biohazardfs auth credentials rotate --dry-run
biohazardfs auth credentials rotate --yes
```

Rules:

- Secrets are never printed unless an explicit one-time reveal command exists and the actor is authorized.
- `auth status` redacts credential material.
- Device sessions must be individually revocable by authorized users/admins.

## Config commands

Config is TOML on disk and JSON through the CLI. Currently implemented:

```bash
biohazardfs config path
biohazardfs config show --redacted
biohazardfs config validate
```

Planned:

```bash
biohazardfs config show
biohazardfs config get <key>
biohazardfs config set <key> <value> --dry-run
biohazardfs config set <key> <value> --yes
biohazardfs config migrate --dry-run
biohazardfs config migrate --yes
```

`config show` is redacted by default in the current scaffold even when `--redacted` is omitted.

Expected config keys include:

```text
profile
server_url
mount.name
mount.path
cache.path
cache.limit_bytes
mutation_policy
output.default
credentials.path
features.*
```

## Schema introspection commands

Agents must be able to discover the command surface without reading Markdown docs.

```bash
biohazardfs schema list
biohazardfs schema command <command-name>
biohazardfs schema event <event-name>
biohazardfs schema error <error-code>
biohazardfs schema config
biohazardfs schema all --output ndjson
```

Examples:

```bash
biohazardfs schema command snapshot.create
biohazardfs schema command file.restore
biohazardfs schema error confirmation_required
biohazardfs schema config
```

Schema output should include:

- command name
- aliases
- input JSON schema
- output `data` schema
- possible errors
- required permissions
- mutation classification
- dry-run/apply requirements
- examples

## MCP surface

`biohazardfs mcp` exposes the same command schema as typed stdio tools.

```bash
biohazardfs mcp
biohazardfs mcp --tools file,snapshot,audit,cache
biohazardfs mcp --profile studio
```

Rules:

- MCP tools are generated from the same schema registry as CLI commands.
- Tool responses use the same envelope semantics.
- The MCP surface must not expose commands hidden from the current actor/profile.
- Mutating tools obey the same mutation policy as CLI commands.

## Canonical command namespaces

The CLI uses canonical namespaced commands. Short aliases may exist for common workflows, but docs and schemas should always identify the canonical command.

### Core/runtime

```bash
biohazardfs version
biohazardfs doctor
biohazardfs doctor --json-deep
biohazardfs smoke run
biohazardfs daemon status
biohazardfs daemon start
biohazardfs daemon stop --yes
biohazardfs daemon restart --yes
biohazardfs daemon logs --limit 100
biohazardfs daemon events --output ndjson
```

### Mount

```bash
biohazardfs mount status
biohazardfs mount attach --path <mount-path>
biohazardfs mount detach --yes
biohazardfs mount list
biohazardfs mount repair --dry-run
biohazardfs mount repair --yes
```

### File namespace

```bash
biohazardfs file stat <path>
biohazardfs file list <path> --limit 100
biohazardfs file history <path>
biohazardfs file versions <path>
biohazardfs file restore --json '{...}' --dry-run
biohazardfs file restore --apply <operation-token>
biohazardfs file delete --json '{...}' --dry-run
biohazardfs file delete --apply <operation-token>
biohazardfs file move --json '{...}' --dry-run
biohazardfs file move --apply <operation-token>
biohazardfs file copy --json '{...}' --dry-run
biohazardfs file copy --yes
biohazardfs file checksum <path>
```

Short aliases:

```bash
biohazardfs ls <path>              # file list
biohazardfs stat <path>            # file stat
biohazardfs history <path>         # file history
biohazardfs versions <path>        # file versions
biohazardfs restore ...            # file restore
```

### Cache namespace

```bash
biohazardfs cache status
biohazardfs cache list --path <path>
biohazardfs cache pin <path> --yes
biohazardfs cache unpin <path> --yes
biohazardfs cache hydrate <path> --yes
biohazardfs cache dehydrate <path> --dry-run
biohazardfs cache dehydrate <path> --yes
biohazardfs cache evict --json '{...}' --dry-run
biohazardfs cache move --path <new-cache-path> --dry-run
biohazardfs cache move --path <new-cache-path> --yes
biohazardfs cache verify
biohazardfs cache repair --dry-run
```

Short aliases:

```bash
biohazardfs pin <path>             # cache pin
biohazardfs unpin <path>           # cache unpin
biohazardfs hydrate <path>         # cache hydrate
biohazardfs dehydrate <path>       # cache dehydrate
```

### Transfer namespace

```bash
biohazardfs transfer list
biohazardfs transfer status <transfer-id>
biohazardfs transfer pause <transfer-id> --yes
biohazardfs transfer resume <transfer-id> --yes
biohazardfs transfer cancel <transfer-id> --dry-run
biohazardfs transfer cancel <transfer-id> --yes
biohazardfs transfer retry <transfer-id> --yes
```

### Snapshot namespace

```bash
biohazardfs snapshot list --limit 50
biohazardfs snapshot create --json '{...}' --dry-run
biohazardfs snapshot create --yes --json '{...}'
biohazardfs snapshot mount <snapshot-id> --path <mount-path>
biohazardfs snapshot unmount <snapshot-id> --yes
biohazardfs snapshot diff <snapshot-a> <snapshot-b> --limit 100
biohazardfs snapshot restore --json '{...}' --dry-run
biohazardfs snapshot restore --apply <operation-token>
```

Short aliases:

```bash
biohazardfs snapshots              # snapshot list
```

### Lock namespace

```bash
biohazardfs lock list --path <path>
biohazardfs lock acquire <path> --yes
biohazardfs lock release <path> --yes
biohazardfs lock status <path>
biohazardfs lock extend <lock-id> --duration 2h --yes
biohazardfs lock break <lock-id> --dry-run
biohazardfs lock break --apply <operation-token>
```

### Conflict namespace

```bash
biohazardfs conflict list --path <path>
biohazardfs conflict show <conflict-id>
biohazardfs conflict resolve --json '{...}' --dry-run
biohazardfs conflict resolve --apply <operation-token>
biohazardfs conflict preserve-all <conflict-id> --yes
```

### Workset namespace

```bash
biohazardfs workset list
biohazardfs workset show <workset-id>
biohazardfs workset activate <workset-id> --yes
biohazardfs workset deactivate <workset-id> --yes
biohazardfs workset sync <workset-id> --dry-run
biohazardfs workset sync <workset-id> --yes
biohazardfs workset create --json '{...}' --dry-run
biohazardfs workset create --yes --json '{...}'
biohazardfs workset update <workset-id> --json '{...}' --dry-run
biohazardfs workset update <workset-id> --yes --json '{...}'
```

### Collaboration/share namespace

```bash
biohazardfs invite create --json '{...}' --dry-run
biohazardfs invite create --yes --json '{...}'
biohazardfs invite list
biohazardfs invite revoke <invite-id> --dry-run
biohazardfs invite revoke --apply <operation-token>

biohazardfs share create --json '{...}' --dry-run
biohazardfs share create --yes --json '{...}'
biohazardfs share list --path <path>
biohazardfs share revoke <share-id> --dry-run
biohazardfs share revoke --apply <operation-token>

biohazardfs grant list --path <path>
biohazardfs grant set --json '{...}' --dry-run
biohazardfs grant set --apply <operation-token>
biohazardfs grant revoke <grant-id> --dry-run
biohazardfs grant revoke --apply <operation-token>

biohazardfs publish create --json '{...}' --dry-run
biohazardfs publish create --yes --json '{...}'
biohazardfs publish list --path <path>
biohazardfs publish revoke <publish-id> --dry-run
biohazardfs publish revoke --apply <operation-token>
```

### Audit namespace

```bash
biohazardfs audit events --path <path> --limit 50
biohazardfs audit events --params '{...}'
biohazardfs audit event <event-id>
biohazardfs audit actor <actor-id> --limit 50
biohazardfs audit export --params '{...}' --output ndjson
```

Audit events should be safe for agents by default: redacted secrets, bounded output, stable event types, and explicit provenance.

### Admin namespace

Admin commands live in the same binary under `admin`, but should be hidden or return permission errors for unauthorized actors.

Initial reserved shape:

```bash
biohazardfs admin user list
biohazardfs admin user show <user-id>
biohazardfs admin device list
biohazardfs admin device revoke <device-id> --dry-run
biohazardfs admin token revoke <token-id> --dry-run
biohazardfs admin retention show
biohazardfs admin retention set --json '{...}' --dry-run
biohazardfs admin support-bundle create --redacted --yes
```

## Error codes

Minimum shared error codes:

```text
invalid_input
schema_validation_failed
confirmation_required
operation_token_required
operation_token_expired
permission_denied
auth_required
device_revoked
not_found
conflict_detected
lock_required
lock_held
network_unavailable
server_unavailable
cache_full
cache_corrupt
transfer_failed
mount_unavailable
unsupported_platform
feature_disabled
internal_error
```

Error objects must include stable `code`, human-readable `message`, and optional structured `details`.

## Exit codes

JSON is the source of truth, but shell exit codes should still be predictable:

```text
0  ok
1  general error / internal error
2  invalid input or schema validation failed
3  auth required or permission denied
4  not found
5  conflict or lock failure
6  network/server unavailable
7  confirmation or operation token required
8  unsupported platform or feature disabled
```

## First implementation slice

The first CLI implementation should not build every command above. It should establish the contract that all later commands follow:

1. Standard JSON response envelope.
2. `schema` registry for implemented commands/errors/config.
3. TOML config read/write/validate.
4. `auth status` with redacted credentials.
5. `doctor` and `smoke run`.
6. `daemon status`.
7. `mount status`.
8. `cache status`, `cache pin`, `cache dehydrate --dry-run`.
9. `file stat`, `file list` against a mock namespace.
10. `mcp` stdio surface for implemented commands.

Do not add mutating filesystem behavior until dry-run, response envelope, input validation, and schema introspection exist.
