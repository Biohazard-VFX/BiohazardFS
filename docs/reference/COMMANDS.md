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

Global flags (implemented):

```bash
biohazardfs <command> \
  --daemon-endpoint <host:port> \
  --config <path> \
  --profile <name> \
  --output json|ndjson|text \
  --fields <field-mask> \
  --cursor <cursor> \
  --source cli|agent|ui|api|server|test \
  --request-id <id> \
  --dry-run \
  --yes
```

`--output text` is for humans only. Tests, agents, scripts, and CI should use default JSON or explicit `--output json` / `--output ndjson`.

`--output` versus `--out` (named distinctly to avoid collision):

- `--output <json|ndjson|text>` is the global response format flag.
- `object get` and `file get` write a downloaded blob to a local file with `--out <path>` (renamed from the earlier `--output`). The CLI refuses to overwrite an existing path.

`--daemon-endpoint` defaults to the dev loopback HTTP JSON-RPC endpoint (`127.0.0.1:47666`) and is the current CLI↔daemon transport; production will move to descriptor-discovered IPC and this flag will be retired. `--limit` exists as a per-command argument on list-style subcommands, not as a global. `--no-color` is not implemented in the current CLI.

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

Status: `auth status`, `auth whoami`, and `auth credentials path` are wired to the daemon spine and return honest scaffold state (no enrolled device, no credentials present, advisory credentials path). `auth enroll`, `auth login`, `auth logout`, and `auth credentials rotate` are wired as CLI subcommands but their daemon backing (`auth.enroll`, `auth.login_token`, `auth.logout`, `auth.rotate_credentials`) returns `method_not_implemented` in the current scaffold. `auth logout` is Admin-class and additionally requires `--yes` or `--dry-run` under the agent-safe profile.

## Config commands

Config is TOML on disk and JSON through the CLI.

Implemented (CLI-local, read resolved TOML from `--config`/`--profile`/env):

```bash
biohazardfs config path
biohazardfs config show --redacted
biohazardfs config validate
```

Scaffold (daemon RPC spine; invoked by the CLI's config subcommands above and by other clients — `config.path`, `config.show`, `config.validate`, and `config.get` all run against the in-memory backend and return scaffold defaults): no separate CLI subcommand is exposed for `config get` yet.

Planned (CLI subcommands not yet present; daemon methods `config.set` and `config.migrate` return `method_not_implemented`):

```bash
biohazardfs config get <key>
biohazardfs config set <key> <value> --dry-run
biohazardfs config set <key> <value> --yes
biohazardfs config migrate --dry-run
biohazardfs config migrate --yes
```

`config show` is redacted by default in the current scaffold even when `--redacted` is omitted; omitting the flag adds a warning that calls this out. The daemon's `config.get` is wired as a spine method but returns `config_key_not_found` for every key because the scaffold holds no configured values.

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

## Namespace commands

Implemented server-backed namespace read command:

```bash
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs namespace children
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs namespace children --parent <node-id> --limit 50
```

Rules:

- `BIOHAZARDFS_SERVER_TOKEN` is the only current server bearer-token source; do not pass server tokens through argv.
- The command reads `server.public_url` from the resolved shared config (`--config`, `--profile`, env overrides, or defaults).
- CLI output uses the `command` envelope with `command = "namespace.children"` even though the server endpoint uses the `operation` envelope.
- Secrets from the server token and database config must not be printed in success or error envelopes.
- The server endpoint returns only live nodes visible to the token's organization.

## Content object commands

Implemented server-backed content-object transfer commands:

```bash
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs object put ./plate.exr
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs object get --sha256 <content-hash> --out ./plate.exr
```

Rules:

- `BIOHAZARDFS_SERVER_TOKEN` is the only current server bearer-token source; do not pass server tokens through argv.
- The commands read `server.public_url` from resolved shared config and currently send bearer tokens only to resolved loopback HTTP server URLs.
- `object.put` reads a bounded local file, uploads it through the server content-object API, and returns `content_hash`, `size_bytes`, `storage_provider`, and `object_key` in the CLI command envelope.
- `object.get` downloads through the server content-object API, verifies the decoded content hash locally, writes the requested object to `--out`, removes the server `content_hex` payload from CLI output, and returns metadata plus `output_path`. The CLI refuses to overwrite an existing `--out` path.
- These commands are content-object primitives, not final user-facing file-version mutation commands.

## Daemon workspace commands

Implemented local daemon workspace runtime commands:

```bash
BIOHAZARDFS_LOCAL_TOKEN=<local-token> biohazardfs daemon workspace-status
BIOHAZARDFS_LOCAL_TOKEN=<local-token> biohazardfs daemon workspace-list --path plates
```

Rules:

- The daemon reads `BIOHAZARDFS_WORKSPACE_ROOT` from its own environment; clients do not pass workspace roots through argv.
- `workspace-list` accepts only relative paths that stay inside the configured workspace root.
- These methods are local runtime inspection primitives for the daemon/Electron/CLI path, not server file APIs.


## FUSE mount command

The FUSE adapter is a separate binary, `biohazardfs-fuse`. It currently exposes two mount modes.

Read-only source-backed mount (Linux; the live mount foundation):

```bash
biohazardfs-fuse mount --source /path/to/workspace --mountpoint /path/to/mountpoint
```

Read-write daemon-backed workspace mount (Linux; the FUSE write path):

```bash
biohazardfs-fuse mount-workspace \
  --daemon-endpoint 127.0.0.1:47666 \
  --local-token "$BIOHAZARDFS_LOCAL_TOKEN" \
  --cache-dir /path/to/cache \
  --mountpoint /path/to/mountpoint
```

Rules:

- For `mount`: the source and mountpoint must already exist and resolve to directories. The adapter canonicalizes both paths before mounting.
- For `mount`: the mounted view is read-only; write, unlink, and rmdir requests are denied with a read-only filesystem error. Directory entries are indexed from regular files and directories under the source root; symlinks and special files are skipped in the MVP to preserve path containment.
- For `mount-workspace`: files hydrate from the daemon via `file.read` on open, writes buffer per file handle, and one complete blob is pushed per flush/fsync via `file.write`. Dirty data is never lost: a write that has not flushed is not acked to the daemon, and dehydrate/evict refuse dirty or pinned cache entries.
- For `mount-workspace`: cache state drives through the legal forward transition path (Absent/Failed → Populating → Ready, Ready → Dirty → Ready on overwrite); illegal transitions are rejected, never papered over.
- Both modes are Linux-only. Other platforms return an explicit `unsupported_platform` error.
- Reproducible smoke proof lives in `scripts/ci/fuse-smoke.sh`; it exercises both `mount` and `mount-workspace` and skips safely when `/dev/fuse` or `fusermount` is unavailable.

## File workflow commands

Two file workflows are implemented. The server-backed metadata workflow (current-version primitives):

```bash
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs file put ./shot001.exr --name shot001.exr
BIOHAZARDFS_SERVER_TOKEN=<token> biohazardfs file get --node <node-id> --out ./shot001.exr
```

Rules:

- `file put` uploads bounded local content through the server, stores the object in RustFS, and records/updates a Postgres file node plus current file version.
- `file get` resolves the current file version by node ID, downloads the server-verified object, verifies the decoded content hash locally, writes it to `--out` (refusing to overwrite an existing path), and strips `content_hex` from CLI output.
- Server bearer tokens still come only from `BIOHAZARDFS_SERVER_TOKEN`; the MVP HTTP client still sends bearer tokens only to resolved loopback HTTP URLs until HTTPS lands.
- This is a smokeable current-version workflow, not the final conflict-resolution/sync protocol.

The daemon-backed file namespace (`file stat`, `file list`, `file history`, `file versions`, `file checksum`, `file read`, `file write`) is wired to the daemon spine; see "Canonical command namespaces" for the full status split. `file restore`, `file delete`, `file move`, and `file copy` are wired as CLI subcommands but their daemon backing returns `method_not_implemented`.

## Schema introspection commands

Agents must be able to discover the command surface without reading Markdown docs. All `schema` subcommands are implemented at the CLI layer and read from the `known_methods` registry, so they return real data without a running daemon:

```bash
biohazardfs schema list
biohazardfs schema command <command-name>
biohazardfs schema event <event-name>
biohazardfs schema error <error-code>
biohazardfs schema config
biohazardfs schema all --output ndjson
```

`biohazardfs commands` is a backward-compatible alias for `schema list`.

Examples:

```bash
biohazardfs schema command snapshot.create
biohazardfs schema command file.restore
biohazardfs schema error confirmation_required
biohazardfs schema config
```

Schema output includes: command name, group, surface, mutation classification, mutation-gate description, summary, and aliases. The richer per-command contract fields below are the target shape; the current descriptor carries the classification + summary that the mutation gate and `tools/list` annotations consume.

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

`biohazardfs mcp serve` exposes a minimal JSON-RPC 2.0 stdio surface.

```bash
biohazardfs mcp serve
```

Status: the stdio seam implements `initialize`, `ping`, and `tools/list`. `tools/list` is generated from the same `known_methods` registry as the CLI, so every registered command appears as a tool with its classification in the annotations. `tools/call` validates the tool name but returns a typed JSON-RPC error (`method_not_implemented`) instructing the caller to run the matching `biohazardfs` CLI subcommand; tool execution is not wired through the CLI tree in this build. The earlier `--tools` / `--profile` filter flags are not implemented.

Rules:

- MCP tools are generated from the same schema registry as CLI commands.
- Tool responses use the same envelope semantics.
- The MCP surface must not expose commands hidden from the current actor/profile.
- Mutating tools obey the same mutation policy as CLI commands.

## Canonical command namespaces

The CLI uses canonical namespaced commands. Short aliases may exist for common workflows, but docs and schemas should always identify the canonical command.

Status legend for the namespaces below (verified against `crates/cli/src/main.rs` and `crates/api-types/src/known_methods.rs`):

- **IMPLEMENTED**: the CLI subcommand exists and its daemon backing runs against the in-memory backend, or the command is resolved CLI-local (schema, version, status, doctor, config). These return real data.
- **SCAFFOLD**: the CLI subcommand exists and dispatches, but the daemon backing returns `method_not_implemented` (periphery), or the command returns a typed stub (for example `daemon start`, `smoke run`, `doctor --json-deep`). The mutation gate, error envelope, and exit code are all real; the body is not.
- **PLANNED**: the command surface below is the contract target but no CLI subcommand is wired yet (for example `config get`, `config set`, `config migrate`, short aliases beyond the ones implemented).

Destructive / admin / data-moving methods are wired as CLI subcommands and gated by the agent-safe policy. Under the current scaffold, running one with `--dry-run` mints a CLI-side operation token and plan (exit 7); running with `--yes` returns `apply_not_wired` (exit 7) and does NOT dispatch to the daemon — the daemon-issued operation-token flow is not yet wired, so the CLI cannot produce a daemon-valid token and declines to call (which would otherwise reject with `operation_token_required`). Running one with neither flag returns `confirmation_required` (exit 7). Read and low-risk mutations proceed end-to-end.

The `--apply <operation-token>` form shown in examples below is **PLANNED**: it depends on a daemon token-issuance RPC that does not exist yet. Until that lands, the only mutation flags are `--dry-run` (plan + CLI-side token) and `--yes` (which surfaces `apply_not_wired`).

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

Status: `version`, `status`, shallow `doctor`, `daemon status`, `daemon methods`, `daemon events`, `daemon workspace-status`, and `daemon workspace-list` are IMPLEMENTED. `doctor --json-deep`, `smoke run`, `daemon start`, `daemon stop`, `daemon restart`, and `daemon logs` are SCAFFOLD: `doctor --json-deep` emits a stub warning, `smoke run` returns `method_not_implemented` and points at `scripts/ci/*-smoke.sh`, and the daemon lifecycle methods (`daemon.shutdown`, `daemon.restart`, `daemon.logs`) are periphery. `daemon stop`/`daemon restart` are also Admin-class and require `--yes` or `--dry-run`.

### Mount

```bash
biohazardfs mount status
biohazardfs mount attach --path <mount-path>
biohazardfs mount detach --yes
biohazardfs mount list
biohazardfs mount repair --dry-run
biohazardfs mount repair --yes
```

Status: `mount status` and `mount list` are IMPLEMENTED (spine). `mount attach`, `mount detach`, and `mount repair` are SCAFFOLD (periphery → `method_not_implemented`). Live attach happens through the `biohazardfs-fuse` adapter binary, not this daemon RPC surface.

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

Status: the daemon spine for `file.stat`, `file.list`, `file.history`, `file.versions`, `file.checksum`, `file.read`, and `file.write` is IMPLEMENTED against the in-memory namespace (resolves by `node_id` / `parent_node_id`). `file write` and `file read` round-trip real content via `content_hex`. `file restore`, `file delete`, `file move`, and `file copy` are SCAFFOLD (periphery → `method_not_implemented`). The short aliases above are PLANNED. Note: the thin `file stat/list/history/versions/checksum` subcommands currently forward a `path` argument while the daemon resolves by `node_id`; the path→node resolution seam is still being wired, so callers that hit the daemon through `--json '{"node_id":"..."}'` get real data while the positional `<path>` forms will round out in a follow-up.

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

Status: `cache status`, `cache list`, `cache pin`, `cache unpin`, `cache hydrate`, `cache dehydrate`, and `cache verify` are IMPLEMENTED. `cache dehydrate` enforces the dirty-data-never-lost and pinned-not-evicted invariants (returns `cache_entry_dirty` / `cache_entry_pinned` instead of removing data). `cache evict`, `cache move`, and `cache repair` are SCAFFOLD (periphery). The short aliases above are PLANNED.

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

Status: `transfer list` and `transfer status` are IMPLEMENTED (spine, against the in-memory transfer records). `transfer pause`, `transfer resume`, `transfer cancel`, and `transfer retry` are SCAFFOLD (periphery).

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

Status: `snapshot list` is IMPLEMENTED (spine; returns an honest empty list because snapshots are server-owned). `snapshot create`, `snapshot mount`, `snapshot unmount`, `snapshot diff`, and `snapshot restore` are SCAFFOLD (periphery). The `snapshots` alias is PLANNED.

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

Status: `lock list`, `lock acquire`, `lock release`, `lock status`, and `lock extend` are IMPLEMENTED (spine, with lazy TTL expiry reporting). `lock break` is SCAFFOLD (periphery, Admin-class).

### Conflict namespace

```bash
biohazardfs conflict list --path <path>
biohazardfs conflict show <conflict-id>
biohazardfs conflict resolve --json '{...}' --dry-run
biohazardfs conflict resolve --apply <operation-token>
biohazardfs conflict preserve-all <conflict-id> --yes
```

Status: `conflict list` and `conflict show` are IMPLEMENTED (spine). `conflict resolve` and `conflict preserve-all` are SCAFFOLD (periphery).

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

Status: `workset list` and `workset show` are IMPLEMENTED (spine; honest empty / `workset_not_found` because worksets are server-owned and the daemon cache is empty in the scaffold). `workset activate`, `workset deactivate`, `workset sync`, `workset create`, and `workset update` are SCAFFOLD (periphery).

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

Status: `invite list`, `share list`, `grant list`, and `publish list` are IMPLEMENTED (spine; honest empty lists). All mutations in this namespace (`invite create/revoke`, `share create/revoke`, `grant set/revoke`, `publish create/revoke`) are SCAFFOLD (periphery). `grant set` and `grant revoke` are Admin-class.

### Audit namespace

```bash
biohazardfs audit events --path <path> --limit 50
biohazardfs audit events --params '{...}'
biohazardfs audit event <event-id>
biohazardfs audit actor <actor-id> --limit 50
biohazardfs audit export --params '{...}' --output ndjson
```

Audit events should be safe for agents by default: redacted secrets, bounded output, stable event types, and explicit provenance.

Status: `audit events`, `audit event`, and `audit actor` are IMPLEMENTED (spine, against the in-memory audit buffer). `audit export` is SCAFFOLD (periphery, Data-moving).

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

Status: every `admin *` subcommand is wired but SCAFFOLD: the daemon backing for the entire `admin.*` group returns `method_not_implemented`. All admin methods are Admin-class, so under the agent-safe profile they require `--dry-run` (mints a CLI plan, exit 7) or `--yes` (dispatches; the daemon then reports `operation_token_required` until the token handoff lands, and `method_not_implemented` once it does).

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
method_not_implemented
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

## Implementation status

The CLI command tree in `crates/cli/src/main.rs` wires the full canonical namespace above (every subcommand is parsed, dispatched, and emits the standard response envelope). What varies is whether the backing produces real data. The status of each command is summarized per-namespace in "Canonical command namespaces" above and verified against `crates/api-types/src/known_methods.rs` and the daemon dispatch table in `crates/daemon/src/lib.rs`.

### Implemented (real behavior, tested)

- Standard JSON response envelope, `--output json|ndjson|text`, ndjson streaming over list-shaped reads, `--fields`/`--cursor` pagination, `--source`/`--request-id` provenance, and the seven global mutation/provenance/output flags.
- Mutation gating under the agent-safe profile: read/low-risk proceed; destructive/admin/data-moving commands require `--dry-run` (CLI mints an operation token + plan, exit 7) or `--yes`. Exit codes 5 (conflict/lock) and 8 (unsupported platform / feature disabled) are mapped.
- `schema list` / `commands` (registry from `known_methods`), `schema command`, `schema event`, `schema error`, `schema config`, `schema all` — resolved CLI-local, no daemon required.
- `version`, `status`, shallow `doctor`, `config path`, `config show --redacted`, `config validate` — CLI-local, read resolved TOML.
- Server-backed `namespace children`, `object put`/`object get`, `file put`/`file get` (content-object and current-version primitives; `--out` writes the local file).
- Daemon workspace runtime: `workspace.status`, `workspace.list`, `daemon.status`, `daemon.health`, `daemon.version`, `daemon.methods`, `daemon.events.subscribe`.
- Daemon file/cache/lock/conflict/transfer/snapshot/workset/collaboration/audit **read spines** plus low-risk mutations (`file.write`, `file.read`, `cache.pin`, `cache.unpin`, `cache.hydrate`, `cache.dehydrate`, `cache.verify`, `lock.acquire`, `lock.release`, `lock.extend`).
- `mcp serve` stdio seam: `initialize`, `ping`, `tools/list` (generated from `known_methods`).
- FUSE: `biohazardfs-fuse mount` (read-only source-backed) and `biohazardfs-fuse mount-workspace` (read-write daemon-backed, hydrate-on-open, write/flush/fsync through `file.write`, dirty-data-never-lost). Linux only; smoke in `scripts/ci/fuse-smoke.sh`.

### Scaffold (typed + wired + tested seam, returns `method_not_implemented` or a typed stub)

- All destructive/admin/data-moving daemon methods are wired as CLI subcommands and pass the mutation gate, but the daemon dispatch routes them to a periphery arm that returns `method_not_implemented` after the operation-token check. This covers: `file.restore/delete/move/copy`, `cache.evict/move/repair`, `transfer.pause/resume/cancel/retry`, `snapshot.create/mount/unmount/diff/restore`, `lock.break`, `conflict.resolve/preserve_all`, `workset.activate/deactivate/sync/create/update`, all `invite/share/grant/publish` mutations, `audit.export`, and the entire `admin.*` group.
- Auth mutations (`auth.enroll`, `auth.login`, `auth.logout`, `auth.credentials rotate`), `config.set`/`config.migrate`, `mount.attach`/`mount.detach`/`mount.repair`, and `daemon.shutdown`/`daemon.restart`/`daemon.logs` are periphery.
- `daemon start`, `smoke run`, and `doctor --json-deep` return typed stubs (`method_not_implemented`) with pointers to where the real behavior will land.
- `mcp serve` `tools/call` validates the tool name but returns a JSON-RPC error instructing the caller to use the matching CLI subcommand; tool execution is not wired through the CLI tree.
- The CLI→daemon operation-token handoff is not wired: the CLI mints dry-run tokens locally, and the daemon validates only tokens it issued itself, so `--yes` on destructive/admin/data-moving methods surfaces `operation_token_required` from the daemon. Read and low-risk mutations execute end-to-end.

### Planned (in the spec, not yet in code)

- `config get`, `config set`, `config migrate` CLI subcommands; the daemon spine for `config.get` exists but `config.set`/`config.migrate` are periphery.
- Short aliases (`ls`, `stat`, `history`, `versions`, `restore`, `pin`, `unpin`, `hydrate`, `dehydrate`, `snapshots`).
- Per-command `--apply <operation-token>` flow and the HumanFriendly mutation profile.
- `--no-color` global, `--limit` as a global, and the `--tools`/`--profile` MCP filter flags.
- Full per-command JSON Schema output (input/output schemas, permissions, examples) in `schema command`; the current descriptor carries classification + summary + mutation gate.
- `config doctor` diagnostic surface and the redacted `smoke run --format json` reporter.

Do not add mutating filesystem behavior beyond the low-risk spine until dry-run, response envelope, input validation, schema introspection, and the daemon operation-token backend are all real.
