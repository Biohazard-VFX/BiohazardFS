# BiohazardFS Daemon API Contract

Status: draft reference
Audience: daemon implementers, CLI implementers, Electron implementers, agent/tooling authors

`biohazardfsd` is the local authority for mount state, cache state, transfer state, local credentials, conflict/lock behavior, and filesystem safety. Electron, the CLI, MCP tools, agents, and tests should all interact with local filesystem state through the daemon unless explicitly using a direct server/headless mode.

## Goals

1. Provide one local API for Electron, CLI, MCP, agents, and tests.
2. Keep sync/filesystem/cache correctness in Rust daemon code, not UI code.
3. Make every operation traceable through request IDs, actor/device/source metadata, and audit events.
4. Keep local access safe: same OS user boundary plus an owner-only local session token.
5. Support structured event streaming for live UI and agent progress without polling.
6. Preserve a clear boundary between local state operations and explicit direct-server/admin operations.

## Non-goals

- The Electron app must not implement filesystem, sync, cache, lock, conflict, or transfer decision logic.
- The daemon API is not a public internet API.
- Local loopback HTTP is not the default production transport unless explicitly enabled.
- Direct server mode must not silently replace daemon-mediated local behavior.

## Daemon lifecycle

On artist workstations, `biohazardfsd` should run as a per-user daemon that auto-starts at login.

Rules:

- The installer registers a user-session service/launch agent/startup task.
- The daemon runs as the logged-in OS user, not as a privileged system daemon by default.
- The daemon owns that user's mounts, cache, local state DB, and local session token.
- CLI and Electron may start the daemon on demand if auto-start failed or the service is not installed.
- The daemon should shut down cleanly on logout, OS shutdown, or explicit user request.
- System-service mode may be added later for render/headless nodes, but it is not the default artist install mode.

## Process and boundary model

```text
Electron Workspace UI
  ↓ local daemon API
biohazardfs CLI ───────┐
biohazardfs mcp ───────┤
agent/tooling ─────────┘
  ↓
biohazardfsd
  ├─ filesystem adapter
  ├─ cache manager
  ├─ transfer manager
  ├─ lock/conflict manager
  ├─ local state DB
  ├─ credential/session manager
  └─ server/control-plane client
```

Normal clients use the daemon for:

- mount attach/detach/status
- placeholder/hydration state
- cache pin/dehydrate/evict/move/verify
- transfer queue and progress
- file metadata as seen by the mounted workspace
- local lock/conflict workflows
- local auth/session state
- event subscriptions
- local diagnostics

The CLI may support explicit direct-server/headless/admin mode for operations that do not need local mount/cache state. This mode must be opt-in and visible in the response envelope metadata.

Examples:

```bash
biohazardfs audit events --server-direct --params '{...}'
biohazardfs admin user list --server-direct
```

Direct-server mode must not be used for local mount/cache/filesystem operations.

## Transport

The primary daemon transport is hybrid:

1. **Preferred production transport:** platform IPC.
   - Linux/macOS: Unix domain socket.
   - Windows: named pipe.
2. **Optional development/integration transport:** loopback HTTP.
   - Binds only to `127.0.0.1` or `[::1]`.
   - Disabled by default unless configured or launched in dev/test mode.
   - Requires the same local authentication as IPC.

Clients discover the daemon endpoint from an owner-only runtime descriptor file.

Example descriptor shape:

```json
{
  "schema_version": "2026-07-daemon-endpoint-v1",
  "pid": 12345,
  "transport": "unix",
  "endpoint": "/run/user/1000/biohazardfs/daemon.sock",
  "http_endpoint": null,
  "token_file": "/run/user/1000/biohazardfs/session.token",
  "started_at": "2026-07-02T18:30:00Z"
}
```

Descriptor and token files must be readable only by the owning OS user.

## Local state store

The daemon uses an owner-only SQLite local state DB.

The local state DB tracks:

- cache index and file hydration state
- transfer queue and retry state
- local file state needed for placeholder/mount behavior
- dry-run operation tokens and plan hashes
- event cursors and client resync checkpoints
- daemon health/problem records
- pending local audit events while offline
- schema/config migration state

Rules:

- The DB file must be readable/writable only by the owning OS user.
- The DB must never store raw long-lived server secrets when an OS keyring is available.
- Dirty/unuploaded file state must survive daemon restart and OS reboot.
- Migrations must be explicit, versioned, and recoverable.
- Support bundles must redact sensitive rows/fields.

## Local authentication

Local daemon clients authenticate with both:

1. OS user ownership/permission boundary.
2. Owner-only local session token.

Rules:

- The daemon refuses clients whose socket/pipe/descriptor ownership does not match the expected OS user.
- The daemon requires a local bearer/session token on every request.
- The local token is distinct from server credentials and must not grant server access by itself.
- Local tokens are redacted from logs, audit events, CLI output, and support bundles.
- Local tokens rotate on daemon restart unless a platform service manager requires a stable session token strategy.
- The daemon must invalidate local tokens on logout and profile switch.

For IPC, the token can be sent in request metadata. For loopback HTTP, use an authorization header:

```http
Authorization: Bearer local_...
```

## Protocol style

The daemon API uses JSON-RPC-like method calls everywhere.

Rules:

- IPC and optional loopback HTTP use the same `method` + `params` request shape.
- HTTP does not expose a separate REST surface for daemon methods.
- Method names are stable dotted strings such as `cache.pin`, `file.history`, and `snapshot.create`.
- This method registry is the source of truth for CLI command mapping, MCP tool generation, schema introspection, and tests.

## Request envelope

Daemon requests use a structured envelope, even when transported over HTTP.

```json
{
  "id": "req_01J...",
  "method": "cache.pin",
  "params": {
    "path": "/Project/Shot010"
  },
  "meta": {
    "source": "cli",
    "actor_hint": null,
    "impersonated_user_id": null,
    "schema_version": "2026-07-daemon-v1"
  }
}
```

Requirements:

- `id` must be generated by the client if possible; daemon generates one if missing.
- `method` is a stable namespaced method string.
- `params` must validate against the method schema.
- `meta.source` must be one of `ui`, `cli`, `agent`, `api`, `server`, or `test`.
- Impersonation requires explicit authorization and must be reflected in audit provenance.

## Response envelope

Daemon responses align with the CLI envelope.

```json
{
  "ok": true,
  "method": "cache.pin",
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
    "schema_version": "2026-07-daemon-v1",
    "server_direct": false
  }
}
```

Error responses use the same envelope:

```json
{
  "ok": false,
  "method": "file.restore",
  "data": null,
  "warnings": [],
  "error": {
    "code": "operation_token_required",
    "message": "This restore requires a dry-run operation token under the current mutation policy.",
    "details": {
      "policy": "agent-safe",
      "classification": "data-moving"
    }
  },
  "meta": {
    "request_id": "req_01J...",
    "timestamp": "2026-07-02T18:30:00Z",
    "actor": null,
    "device": null,
    "source": "agent",
    "schema_version": "2026-07-daemon-v1",
    "server_direct": false
  }
}
```

## Event stream

The daemon exposes a one-way structured event stream first. Bidirectional streaming can be added later if needed.

Supported first forms:

- IPC/pipe NDJSON stream.
- Loopback HTTP Server-Sent Events or NDJSON stream in dev/integration mode.
- CLI bridge: `biohazardfs daemon events --output ndjson`.

Event envelope:

```json
{
  "type": "transfer.progress",
  "id": "evt_01J...",
  "timestamp": "2026-07-02T18:30:00Z",
  "data": {
    "transfer_id": "xfer_...",
    "path": "/Project/Shot010/plate.exr",
    "direction": "download",
    "bytes_done": 10485760,
    "bytes_total": 52428800,
    "state": "running"
  },
  "meta": {
    "request_id": null,
    "actor_id": null,
    "device_id": "dev_...",
    "schema_version": "2026-07-events-v1"
  }
}
```

Initial event families:

```text
daemon.started
daemon.stopping
daemon.health_changed
auth.changed
mount.attached
mount.detached
mount.health_changed
file.changed
cache.state_changed
cache.quota_warning
transfer.queued
transfer.progress
transfer.completed
transfer.failed
lock.changed
conflict.detected
conflict.resolved
snapshot.created
snapshot.mounted
audit.event_recorded
warning.raised
```

Rules:

- Events must be bounded and machine-readable.
- Events must not contain secrets.
- Events must include stable type names.
- Event schemas are introspectable via CLI schema commands and daemon schema methods.
- Clients must tolerate dropped event streams by resyncing through state/list methods.

## Schema introspection

The daemon owns or consumes the same schema registry as the CLI.

Methods:

```text
schema.list
schema.method
schema.event
schema.error
schema.config
schema.all
```

Schema responses should include:

- method name
- input params schema
- response data schema
- event schemas emitted by the method
- possible error codes
- required permissions
- mutation classification
- dry-run/apply requirements

## Method groups

The exact method schemas will evolve, but the group boundaries are product requirements.

### Daemon/runtime

```text
daemon.status
daemon.health
daemon.version
daemon.shutdown
daemon.restart
daemon.logs
daemon.events.subscribe
```

### Auth/session

```text
auth.status
auth.enroll
auth.login_token
auth.logout
auth.whoami
auth.credentials_path
auth.rotate_credentials
```

### Config

```text
config.path
config.show
config.get
config.set
config.validate
config.migrate
```

### Mount

```text
mount.status
mount.attach
mount.detach
mount.list
mount.repair
```

### File

```text
file.stat
file.list
file.history
file.versions
file.restore
file.delete
file.move
file.copy
file.checksum
```

### Cache

```text
cache.status
cache.list
cache.pin
cache.unpin
cache.hydrate
cache.dehydrate
cache.evict
cache.move
cache.verify
cache.repair
```

### Transfer

```text
transfer.list
transfer.status
transfer.pause
transfer.resume
transfer.cancel
transfer.retry
```

### Snapshot

```text
snapshot.list
snapshot.create
snapshot.mount
snapshot.unmount
snapshot.diff
snapshot.restore
```

### Lock

```text
lock.list
lock.acquire
lock.release
lock.status
lock.extend
lock.break
```

### Conflict

```text
conflict.list
conflict.show
conflict.resolve
conflict.preserve_all
```

### Workset

```text
workset.list
workset.show
workset.activate
workset.deactivate
workset.sync
workset.create
workset.update
```

### Collaboration/share

```text
invite.create
invite.list
invite.revoke
share.create
share.list
share.revoke
grant.list
grant.set
grant.revoke
publish.create
publish.list
publish.revoke
```

### Audit

```text
audit.events
audit.event
audit.actor
audit.export
```

### Admin

Admin methods exist in the same daemon/server method namespace but must be permission-gated.

```text
admin.user.list
admin.user.show
admin.device.list
admin.device.revoke
admin.token.revoke
admin.retention.show
admin.retention.set
admin.support_bundle.create
```

## Mutation and dry-run behavior

Daemon mutation behavior must follow `docs/reference/COMMANDS.md` safety profiles.

- Fresh installs default to `agent-safe`.
- The daemon stores and enforces the selected mutation policy.
- The daemon classifies methods as read, low-risk mutation, destructive, admin, or data-moving.
- Destructive/admin/data-moving methods require dry-run operation tokens in `agent-safe` mode.
- Operation tokens bind the validated params, actor, device, source, mutation policy, plan hash, and expiry.
- Applying an operation token with changed params must fail.

Operation token data model:

```json
{
  "operation_token": "op_01J...",
  "method": "file.restore",
  "params_hash": "sha256:...",
  "plan_hash": "sha256:...",
  "actor_id": "usr_...",
  "device_id": "dev_...",
  "source": "agent",
  "classification": "data-moving",
  "expires_at": "2026-07-02T18:45:00Z"
}
```

## Offline behavior

The daemon should support full optimistic offline mode.

Offline mode means authorized users can continue working against the mounted namespace even when the daemon cannot reach the server/control plane or storage backend.

Rules:

- Cached and pinned files remain readable.
- Already-known namespace metadata remains visible with clear degraded/offline state.
- Writes are accepted locally when the daemon can prove the path was previously writable or has an offline grant/workset policy allowing optimistic creation.
- New files, edits, renames, deletes, lock requests, and publish intents are queued locally with durable operation records.
- Dirty/unuploaded data must never be evicted.
- The daemon must preserve enough metadata to replay or reconcile operations after reconnect.
- Reconnect must never silently overwrite remote changes.
- If remote state changed while local offline edits also occurred, the daemon always preserves both sides and creates conflict records.
- Conflicts created by offline divergence are preserved as conflicts with every version recoverable.
- Automatic merge of divergent file content is out of scope for MVP.
- Audit events generated offline are marked as locally recorded and later server-acknowledged.
- UI/CLI/agents must be able to query offline queue state and sync/reconciliation status.

Offline mode adds complexity and must be gated by tests before MVP claims.

## Audit provenance

Every meaningful daemon-mediated operation should be able to produce an audit event. At minimum, audit provenance includes:

```text
actor
impersonated user, if any
device
source: ui | cli | agent | api | server | test
method
request_id
path/object IDs
timestamp
result
```

The daemon may buffer local audit events while offline, but must preserve order and retry safely.

## Direct server mode boundary

Explicit direct-server/headless/admin mode is allowed for operations that do not depend on local mount/cache/filesystem state.

Rules:

- Must require an explicit flag, config profile, or admin subcommand mode.
- Must set `meta.server_direct = true` in responses.
- Must not be used for mount/cache/local transfer queue/filesystem adapter operations.
- Must use server credentials, not local daemon session tokens.
- Must preserve the same response envelope and schema system.

Allowed examples:

```text
audit.events
admin.user.list
admin.device.revoke
invite.create
share.revoke
snapshot.list
```

Disallowed examples:

```text
mount.attach
cache.pin
cache.dehydrate
transfer.pause
file.stat against local placeholder state
```

## Initial implementation slice

The first daemon API implementation should establish the safety and introspection substrate before real filesystem mutation:

1. Endpoint discovery descriptor.
2. IPC transport on the first dogfood platform.
3. Optional loopback HTTP for tests/dev.
4. Local session token validation.
5. Standard request/response envelope.
6. Schema registry for implemented methods/events/errors/config.
7. `daemon.status`, `daemon.health`, `daemon.version`.
8. `config.path`, `config.show`, `config.validate`.
9. `auth.status` with redaction.
10. `mount.status` against mock state.
11. `cache.status`, `cache.pin`, `cache.dehydrate --dry-run` against mock state.
12. `file.stat`, `file.list` against mock namespace.
13. `daemon.events.subscribe` NDJSON/SSE stream.

Do not implement destructive local filesystem behavior before local auth, schema validation, dry-run planning, response envelopes, and audit provenance are implemented.
