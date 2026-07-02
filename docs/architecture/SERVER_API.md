# BiohazardFS Server API Scaffold

Status: draft reference
Audience: server implementers, client/daemon implementers, operators, automation agents

This document captures the currently runnable server API scaffold. It is intentionally small, but it must not drift from the broader server architecture contract.

## Scope

The current server is a running foundation only. It does not yet implement auth, metadata persistence, transfer authorization, migrations, workers, or object-storage operations.

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
biohazardfs-server serve --addr 127.0.0.1:8080
biohazardfs-server worker
biohazardfs-server migrate
biohazardfs-server health
biohazardfs-server version
biohazardfs-server config
```

Current non-serve modes are scaffold/no-op modes that print JSON and exit successfully.

## Current HTTP endpoints

When running `biohazardfs-server serve`, the scaffold exposes:

| Endpoint | Operation | Purpose |
| --- | --- | --- |
| `GET /healthz` | `server.health` | Liveness check |
| `GET /readyz` | `server.ready` | Readiness check |
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
4. Validates `health`, `version`, `migrate`, and `worker` CLI modes.

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

The server still uses scaffolded dependency checks; it does not connect to Postgres or object storage yet.

## Next required server work

Before claiming a real server MVP, implement:

1. TOML-backed shared typed config loading and validation beyond the current env-backed scaffold.
2. Database connection and migration records.
3. RustFS/S3-compatible object-store config validation and bucket checks.
4. Auth/device enrollment endpoints.
5. Metadata schema migrations.
6. Server-side operation/idempotency records.
7. Transfer authorization skeleton.
8. Integration tests using Postgres and S3-compatible storage.
