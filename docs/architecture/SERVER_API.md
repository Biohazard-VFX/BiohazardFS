# BiohazardFS Server API Scaffold

Status: draft reference
Audience: server implementers, client/daemon implementers, operators, automation agents

This document captures the currently runnable server API scaffold. It is intentionally small, but it must not drift from the broader server architecture contract.

## Scope

The current server is a running foundation only. It does not yet implement full transfer authorization sessions, workers, sync conflict resolution, or daemon sync integration. It does include the Postgres metadata baseline (migrations 001 and 003 together covering the full MVP record set), an authenticated read-only namespace listing endpoint over Postgres, server-side RustFS/S3-compatible bucket check/ensure admin commands, authenticated content-object PUT/GET primitives backed by RustFS, a metadata-backed current-version file PUT/GET workflow, and a Wave 2 read/submit spine over the metadata tables (locks, conflicts, operations, trash, audit events, devices, projects, worksets). Periphery mutation, replay, snapshot, transfer, grant, share, publish, invite, node-mutation, and auth-enrollment routes are typed and wired but return `operation_not_implemented` until their semantics are built.

It does establish:

- one runnable `biohazardfs-server` binary
- distinct server response envelopes
- a distinct server schema version
- HTTP health/readiness/version/status endpoints
- first authenticated read-only namespace metadata endpoint
- server-side RustFS/S3-compatible signed bucket check/ensure commands
- authenticated content-object PUT/GET primitives backed by RustFS
- authenticated current-version file PUT/GET primitives backed by Postgres metadata plus RustFS content
- migration `003_metadata_baseline.sql` extending the 001 baseline with the full MVP metadata table set: `devices`, `projects`, `worksets`, `workset_rules`, `retention_policies`, `snapshots`, `locks`, `conflicts`, `grants`, `shares`, `publishes`, `invites`, `trash_records`
- authenticated Wave 2 read/submit spine over Postgres: `locks` acquire/list/release, `conflicts` list/show, `operations` submit with idempotency replay, `trash` list, `audit/events`, `devices` list, `projects` list, `worksets` list
- a typed periphery route surface (snapshots, transfers, grants, shares, publishes, invites, node mutation, auth enrollment, `operations.replay`, `trash.restore/purge`, `audit.export`, `devices.revoke`, `projects.create`, `worksets.create`) wired to return `operation_not_implemented`
- server modes for `serve`, `worker`, `migrate`, `health`, `version`, `admin`, `object-store`, and `config`
- Docker, Compose, Helm, and CI smoke validation

## Schema version

Current server schema version:

```text
2026-07-server-v1
```

This is separate from:

- CLI command schema: `2026-07-commands-v1`
- daemon API schema: `2026-07-daemon-v1`
- daemon event schema: `2026-07-events-v1`

## Response envelope

Server HTTP endpoints return a server envelope with `operation`, not `command` or `method`.

```json
{
  "ok": true,
  "operation": "server.status",
  "data": {},
  "warnings": [],
  "error": null,
  "meta": {
    "request_id": "req_...",
    "timestamp": "2026-07-02T18:30:00Z",
    "source": "server",
    "schema_version": "2026-07-server-v1",
    "api_version": "v1"
  }
}
```

Rules:

- `operation` identifies the server operation or endpoint result.
- `meta.schema_version` identifies the server envelope schema.
- `meta.api_version` identifies the public HTTP API version.
- Timestamps are RFC3339 UTC.
- Errors use the same envelope with `ok: false`.

## Current binary modes

```bash
biohazardfs-server --config /path/to/config.toml serve --addr 127.0.0.1:8080
biohazardfs-server --config /path/to/config.toml worker
biohazardfs-server --config /path/to/config.toml migrate
biohazardfs-server --config /path/to/config.toml health
biohazardfs-server version
biohazardfs-server --config /path/to/config.toml admin
biohazardfs-server --config /path/to/config.toml object-store check
biohazardfs-server --config /path/to/config.toml object-store ensure-bucket
biohazardfs-server --config /path/to/config.toml config
```

`worker` remains a scaffold mode. `admin` prints a redacted readiness envelope (database and object-store configured vs missing) without echoing connection strings or credentials, and does not perform admin work yet. `migrate` resolves the shared config via `--config`, `--profile`, `BIOHAZARDFS_CONFIG_FILE`, `BIOHAZARDFS_CONFIG_DIR`, and environment overrides. It requires a database URL from `[database].url` or `BIOHAZARDFS_DATABASE_URL`, applies bundled Postgres migrations, prints a server JSON envelope, and exits nonzero with a redacted JSON error envelope when the database URL is missing or unusable. The database URL is never accepted directly through argv.

`object-store check` and `object-store ensure-bucket` resolve `[object_store]` / `BIOHAZARDFS_OBJECT_STORE_*` config, sign path-style S3-compatible requests server-side, and print redacted server envelopes. They require endpoint, bucket, access key ID, and secret access key from config/env rather than argv. The current MVP object-store client supports `http://` endpoints for internal/self-hosted RustFS paths only; TLS support is still future work.

## Current HTTP endpoints

When running `biohazardfs-server serve`, the scaffold exposes:

| Endpoint | Operation | Purpose |
| --- | --- | --- |
| `GET /healthz` | `server.health` | Liveness check |
| `GET /readyz` | `server.ready` | Readiness check; returns degraded/not-ready when the resolved shared config contains a database URL and the server cannot verify the latest migration |
| `GET /version` | `server.version` | Version and schema info |
| `GET /api/v1/status` | `server.status` | Server/control-plane status |
| `GET /api/v1/namespace/children` | `server.namespace.children` | Authenticated live child-node listing for the caller's organization |
| `PUT /api/v1/objects/content` | `server.objects.content.put` | Authenticated bounded content-object upload backed by RustFS |
| `GET /api/v1/objects/content?sha256=<hash>` | `server.objects.content.get` | Authenticated content-object fetch by SHA-256 |
| `POST /api/v1/nodes/mkdir` | `server.nodes.mkdir` | Authenticated idempotent directory creation under the caller's organization |
| `PUT /api/v1/files/content?name=<name>[&parent_node_id=<id>][&base_version_id=<id>]` | `server.files.content.put` | Authenticated current-version file upload: store content and record/update metadata |
| `GET /api/v1/files/content?node_id=<id>` | `server.files.content.get` | Authenticated current-version file download by metadata node ID |

`GET /api/v1/namespace/children` requires `Authorization: Bearer <token>`. The server stores and compares token hashes in the globally unique `tokens.secret_hash` column using the current `sha256:<hex>` MVP format. The token must be active, unexpired, unrevoked, attached to an active user and organization, and include one of `namespace:read`, `namespace:*`, `server:read`, or `*` in its JSON `scopes` array. The endpoint returns only live nodes in that authenticated org and accepts optional query params:

- `parent=<node_id>` or `parent_node_id=<node_id>`; omitted means root children where `parent_node_id IS NULL`
- `limit=<n>`; default `100`, max `500`

`PUT /api/v1/objects/content` requires one of `object:write`, `object:*`, `file:write`, `server:write`, or `*`. The server reads a bounded request body, computes the SHA-256 hash, signs a path-style RustFS/S3-compatible PUT, stores the object under the caller organization's deterministic content key, and returns content hash, size, provider, and object key. The MVP upload limit is currently 1 MiB.

`GET /api/v1/objects/content?sha256=<hash>` requires one of `object:read`, `object:*`, `file:read`, `file:*`, `server:read`, or `*`. It fetches the deterministic object from RustFS, verifies the SHA-256 hash, and returns a JSON response with `content_hex`. This hex payload is intentionally inefficient but binary-safe for the first smokeable primitive; client/daemon file workflows should replace it with a streaming or presigned transfer contract later.

`POST /api/v1/nodes/mkdir` requires one of `node:write`, `node:*`, `file:write`, `file:*`, `server:write`, or `*`. The JSON body is `{ "parent_node_id": null | "node_...", "name": "folder" }`. The operation is idempotent for an existing directory with the same parent/name and rejects file/directory kind conflicts.

`PUT /api/v1/files/content?name=<name>[&parent_node_id=<id>][&source=cli][&base_version_id=<id>]` requires one of `file:write`, `file:*`, `server:write`, or `*`. The server stores the bounded content object in RustFS, then records a content manifest, creates or updates a live file node under the authenticated organization, creates a file version, and points the node at that current version. When `base_version_id` is supplied for an existing file, the server rejects the write with `version_conflict` unless the current server version still matches that base. This first slice intentionally skips final operation/idempotency/conflict semantics.

`GET /api/v1/files/content?node_id=<id>` requires one of `file:read`, `file:*`, `server:read`, or `*`. It resolves the node current version from Postgres, verifies that the stored object key matches the deterministic org-scoped content key, fetches the object from RustFS, verifies the hash, and returns metadata plus `content_hex`.

Compatibility aliases currently exist for early chart probes:

- `GET /health`
- `GET /ready`

Unknown paths return `404` with `error.code = "not_found"`.

Non-GET requests return `405` with `error.code = "method_not_allowed"`.

## Wave 2 metadata spine endpoints

Migration `003_metadata_baseline.sql` extends the 001 baseline with the full MVP metadata table set (`devices`, `projects`, `worksets`, `workset_rules`, `retention_policies`, `snapshots`, `locks`, `conflicts`, `grants`, `shares`, `publishes`, `invites`, `trash_records`). Over that baseline, the server exposes a read/submit spine backed by real Postgres queries:

| Endpoint | Operation | Status |
| --- | --- | --- |
| `GET /api/v1/locks` | `server.locks.list` | Implemented |
| `POST /api/v1/locks` | `server.locks.acquire` | Implemented |
| `DELETE /api/v1/locks?lock_id=<id>` | `server.locks.release` | Implemented |
| `GET /api/v1/conflicts` | `server.conflicts.list` | Implemented |
| `GET /api/v1/conflicts?conflict_id=<id>` | `server.conflicts.show` | Implemented |
| `POST /api/v1/operations` | `server.operations.submit` | Implemented (idempotency replay) |
| `GET /api/v1/trash` | `server.trash.list` | Implemented |
| `GET /api/v1/audit/events` | `server.audit.events` | Implemented |
| `GET /api/v1/devices` | `server.devices.list` | Implemented |
| `GET /api/v1/projects` | `server.projects.list` | Implemented |
| `GET /api/v1/worksets` | `server.worksets.list` | Implemented |

Each spine route requires `Authorization: Bearer <token>`, resolves the authenticated subject against the globally unique `tokens.secret_hash` column, enforces org scoping, and checks a route-specific scope before touching the store: `lock:read`/`lock:write`, `conflict:read`, `operation:write`, `trash:read`, `audit:read`, `device:read`, `project:read`, `workset:read`, or `*`. List endpoints accept optional filters (`status`, plus a parent/project/node filter where it applies) and a `limit` (default `100`, max `500`), and return timestamps as explicit RFC3339 UTC. `locks.acquire` validates `kind` (`edit`/`admin`/`publish`/`restore`) and `ttl_seconds` (max 86400) and preflights `node_id`/`owner_device_id` existence when supplied. `operations.submit` validates `kind`, `source`, optional `node_id`/`base_version_id`/`device_id` existence, and replays the prior recorded operation when a resubmitted `idempotency_key` matches rather than creating a duplicate.

Status codes follow the rest of the API: `401` `auth_required`/`auth_invalid`, `403` `auth_scope_missing`, `400` for invalid filters/IDs/bodies, `404` `*_not_found` for client-supplied IDs that do not exist in the org, `503` `database_url_missing` when no database is configured, and `503` `metadata_store_unavailable`/`*_store_unavailable` when Postgres is unreachable. Unit tests cover dispatch routing, auth-required, body validation, and fail-closed-without-database behavior for each spine route; live-Postgres end-to-end coverage of the spine route matrix is not yet wired into the smoke script (`scripts/ci/server-db-smoke.sh` currently verifies the migration table set and the `namespace/children` endpoint).

## Periphery route surface (scaffold)

The following routes are typed, registered in `is_known_server_route`, and bound to canonical operation names in `crates/api-types/src/known_methods.rs`, but their handlers return `501` with `error.code = "operation_not_implemented"`. They are present so clients, the daemon, and agents can discover the intended surface and fail fast with a stable code:

- `server.snapshots.list/create/mount/restore` — `GET`/`POST /api/v1/snapshots`, `POST /api/v1/snapshots/mount`, `POST /api/v1/snapshots/restore`
- `server.transfers.create/commit` — `POST /api/v1/transfers`, `POST /api/v1/transfers/commit`
- `server.grants.list/set/revoke` — `GET`/`POST`/`DELETE /api/v1/grants`
- `server.shares.list/create/revoke` — `GET`/`POST`/`DELETE /api/v1/shares`
- `server.publishes.list/create/revoke` — `GET`/`POST`/`DELETE /api/v1/publishes`
- `server.invites.list/create/revoke` — `GET`/`POST`/`DELETE /api/v1/invites`
- `server.nodes.stat/symlink/move/copy/delete` — `GET /api/v1/nodes/stat`, `POST /api/v1/nodes/{symlink,move,copy}`, `DELETE /api/v1/nodes` (`server.nodes.mkdir` is implemented as the first node mutation spine)
- `server.auth.device.enroll`, `server.auth.login_token` — `POST /api/v1/auth/device/enroll`, `POST /api/v1/auth/login_token`
- `server.operations.replay` — `POST /api/v1/operations/replay`
- `server.trash.restore/purge` — `POST /api/v1/trash/restore`, `POST /api/v1/trash/purge`
- `server.audit.export` — `GET /api/v1/audit/export`
- `server.devices.revoke` — `POST /api/v1/devices/revoke`
- `server.projects.create` — `POST /api/v1/projects`
- `server.worksets.create` — `POST /api/v1/worksets`

A known route hit with an unsupported method still returns `405 method_not_allowed`; unknown paths still return `404 not_found`.

## Local smoke validation

```bash
scripts/ci/server-smoke.sh
```

This script:

1. Builds `biohazardfs-server`.
2. Starts `serve` on a local test port.
3. Validates `/healthz`, `/readyz`, `/version`, and `/api/v1/status`.
4. Validates `health`, `version`, `worker`, and the redacted missing-database error path for `migrate`.

```bash
scripts/ci/server-db-smoke.sh
```

This script uses live Postgres to validate migrations, TOML-only DB config, DB-backed readiness, smoke-seeded bearer-token auth, `GET /api/v1/namespace/children` org/deleted-node filtering, and `biohazardfs namespace children` CLI behavior.

```bash
scripts/ci/object-store-smoke.sh
```

This script uses live RustFS to validate signed server-side object-store bucket check/ensure behavior and verifies access key material is not printed.

```bash
scripts/ci/server-transfer-smoke.sh
```

This script uses live Postgres, live RustFS, and the HTTP server to validate authenticated content-object upload/download round trips, metadata-backed file put/get round trips, CLI object/file workflows, and verifies bearer/database/object-store secrets are not printed.

## Docker and Compose

Docker image build:

```bash
docker build -f deploy/docker/server/Dockerfile -t biohazardfs-server:local .
```

Development Compose config validation:

```bash
docker compose -f deploy/compose/dev/docker-compose.yml config --quiet
```

The dev Compose stack includes:

- `server`
- `postgres`
- `object-store` using RustFS, the canonical BiohazardFS self-hosted object-store default

The server migration, namespace-read, content-object, and metadata-backed file paths connect to Postgres when the resolved shared config contains `[database].url` or `BIOHAZARDFS_DATABASE_URL`. The dev Compose stack uses `BIOHAZARDFS_DATABASE_URL` for container wiring, while CI also proves TOML-only migration/readiness through `scripts/ci/server-db-smoke.sh`. The object-store admin check path signs S3-compatible requests against RustFS through `scripts/ci/object-store-smoke.sh`; the first server-mediated content-object and file transfer paths are covered by `scripts/ci/server-transfer-smoke.sh`.

## Next required server work

The Wave 2 spine closes the first loop on metadata read/submit: `locks`, `conflicts`, `operations.submit` (with idempotency), `trash`, `audit.events`, `devices`, `projects`, and `worksets` list/show are now Postgres-backed, and migration 003 lands the full metadata table set. Remaining work before claiming a real server MVP, roughly in priority order:

1. Auth/device enrollment and bootstrap endpoints (`server.auth.device.enroll`, `server.auth.login_token` are currently scaffold).
2. Node mutation APIs (`server.nodes.mkdir/symlink/move/copy/delete`, `server.nodes.stat`) over the existing node/file-version foundation, with operation/idempotency linkage.
3. Operation replay engine: submitted operations record but do not yet apply (`server.operations.replay` is currently scaffold).
4. Conflict resolution and daemon sync integration (`server.conflicts.list/show` are read-only on the server; conflict creation from replayed operations, `server.conflicts.resolve`, and daemon reconciliation are not yet wired).
5. Snapshot, grant, share, publish, invite, and admin destructive paths (`server.snapshots.*`, `server.grants.*`, `server.shares.*`, `server.publishes.*`, `server.invites.*`, `server.devices.revoke`, `server.projects.create`, `server.worksets.create`, `server.trash.restore/purge`, `server.audit.export` are currently scaffold).
6. Transfer authorization/session skeleton beyond the first bearer-scoped content-object primitive (`server.transfers.create/commit` are currently scaffold).
7. Replace hex-body MVP transfers with streaming or presigned transfer sessions; TLS support for the object-store client remains future work.
