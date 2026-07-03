# BiohazardFS Server API Scaffold

Status: draft reference
Audience: server implementers, client/daemon implementers, operators, automation agents

This document captures the currently runnable server API scaffold. It is intentionally small, but it must not drift from the broader server architecture contract.

## Scope

The current server is a running foundation only. It does not yet implement auth, metadata persistence APIs, transfer authorization, workers, or object-storage operations. It does include the first Postgres migration foundation for MVP metadata tables.

It does establish:

- one runnable `biohazardfs-server` binary
- distinct server response envelopes
- a distinct server schema version
- HTTP health/readiness/version/status endpoints
- server modes for `serve`, `worker`, `migrate`, `health`, `version`, and `config`
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
biohazardfs-server --config /path/to/config.toml config
```

`worker` remains a scaffold mode. `migrate` resolves the shared config via `--config`, `--profile`, `BIOHAZARDFS_CONFIG_FILE`, `BIOHAZARDFS_CONFIG_DIR`, and environment overrides. It requires a database URL from `[database].url` or `BIOHAZARDFS_DATABASE_URL`, applies bundled Postgres migrations, prints a server JSON envelope, and exits nonzero with a redacted JSON error envelope when the database URL is missing or unusable. The database URL is never accepted directly through argv.

## Current HTTP endpoints

When running `biohazardfs-server serve`, the scaffold exposes:

| Endpoint | Operation | Purpose |
| --- | --- | --- |
| `GET /healthz` | `server.health` | Liveness check |
| `GET /readyz` | `server.ready` | Readiness check; returns degraded/not-ready when the resolved shared config contains a database URL and the server cannot verify the latest migration |
| `GET /version` | `server.version` | Version and schema info |
| `GET /api/v1/status` | `server.status` | Server/control-plane status |

Compatibility aliases currently exist for early chart probes:

- `GET /health`
- `GET /ready`

Unknown paths return `404` with `error.code = "not_found"`.

Non-GET requests return `405` with `error.code = "method_not_allowed"`.

## Local smoke validation

```bash
scripts/ci/server-smoke.sh
```

This script:

1. Builds `biohazardfs-server`.
2. Starts `serve` on a local test port.
3. Validates `/healthz`, `/readyz`, `/version`, and `/api/v1/status`.
4. Validates `health`, `version`, `worker`, and the redacted missing-database error path for `migrate`.

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

The server migration command connects to Postgres when the resolved shared config contains `[database].url` or `BIOHAZARDFS_DATABASE_URL`. The dev Compose stack uses `BIOHAZARDFS_DATABASE_URL` for container wiring, while CI also proves TOML-only migration/readiness through `scripts/ci/server-db-smoke.sh`. Object storage remains scaffolded and no object/file APIs are implemented yet.

## Next required server work

Before claiming a real server MVP, implement:

1. RustFS/S3-compatible object-store config validation and bucket checks.
2. Auth/device enrollment endpoints.
3. Server-side operation/idempotency APIs over the metadata foundation.
4. Transfer authorization skeleton.
5. Integration tests using live Postgres and S3-compatible storage.
