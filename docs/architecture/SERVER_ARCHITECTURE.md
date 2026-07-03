# BiohazardFS Server Architecture Contract

Status: draft reference
Audience: server/control-plane implementers, client/daemon implementers, operators, packaging maintainers

The BiohazardFS server/control plane is part of the public open-source BiohazardFS repository from the start. Self-hosting is a first-class product requirement, not an afterthought.

## Core decisions

- The server/control plane lives in the public BiohazardFS repo from the beginning.
- The repo should include Docker packaging for the server.
- The repo should include a Helm chart for Kubernetes/self-hosted deployment.
- Start as a modular monolith, not microservices.
- One server codebase can run API, worker, migration, and admin modes.
- PostgreSQL is the source of truth for metadata, audit, operations, locks, conflicts, grants, snapshots, and server state.
- S3-compatible object storage stores content manifests and file data.
- Normal clients never receive permanent storage or database credentials.
- The server issues short-lived scoped transfer authorization.
- Server validates every daemon/client operation; it never blindly trusts the daemon.

## Public repository shape

Expected future shape:

```text
crates/
  server/          # server/control-plane binary and modules
  api-types/       # shared request/response/event/schema types
  core/            # shared domain logic and invariants
deploy/
  docker/
    Dockerfile
    entrypoint.sh
  helm/
    biohazardfs/
      Chart.yaml
      values.yaml
      templates/
server/
  migrations/      # versioned SQL migrations, if not under crates/server
  seeds/           # optional dev/demo seeds
```

The exact layout can evolve, but Docker and Helm assets should stay in-repo and be maintained with the server code.

## Runtime model

Start with one server binary that supports multiple modes.

Current scaffold modes:

```bash
biohazardfs-server serve --addr 127.0.0.1:8080
biohazardfs-server worker
biohazardfs-server migrate
biohazardfs-server health
biohazardfs-server version
```

Planned future admin surface:

```bash
biohazardfs-server admin ...
```

Recommended deployment modes:

```text
API deployment       biohazardfs-server serve
Worker deployment    biohazardfs-server worker
Migration job        biohazardfs-server migrate
Admin commands       biohazardfs-server admin ...
```

For simple self-hosted installs, API and worker may run in one process if explicitly configured. Production deployments should be able to run them separately.

## Logical modules

The modular monolith should be internally separated into modules/services:

```text
HTTP/API gateway
Auth/session/device service
Org/user/project/workset service
Namespace/metadata service
Version/snapshot service
Lock/conflict service
Operation replay/reconciliation service
Transfer authorization service
Audit service
Share/invite/grant service
Retention/trash service
Background worker scheduler
Migration runner
Admin/operator surface
```

These are logical modules, not separate network services for MVP.

## External dependencies

Required dependencies:

```text
PostgreSQL
S3-compatible object storage
```

Optional/future dependencies:

```text
Redis or queue backend, if in-process/Postgres-backed jobs are insufficient
OIDC/SAML identity provider
Project tracker integration
Email/notification provider
Metrics/log aggregation
```

The self-hosted MVP should not require proprietary/private Biohazard infrastructure.

## API style

The server exposes an HTTP JSON API.

The current running scaffold is documented in [`SERVER_API.md`](SERVER_API.md). It exposes `/healthz`, `/readyz`, `/version`, and `/api/v1/status` using the `2026-07-server-v1` envelope.

Rules:

- Use stable JSON request/response envelopes aligned with daemon/CLI envelope patterns.
- Use explicit API versioning.
- Use idempotency keys for mutating/replayed operations.
- Use pagination and field masks for large reads.
- Use stable error codes.
- Avoid browser-only flows for machine/agent/headless use.

Future gRPC or other transports can be added only after the HTTP JSON API is stable enough to support clients and agents.

## Authentication and authorization

The server owns:

- org/studio identity boundary
- user identity records
- device enrollment and revocation
- API/device token validation
- invite/device-code flows
- grant/permission evaluation
- share access
- admin authorization

Rules:

- Devices are revocable independently.
- Tokens are scoped and revocable.
- Store token hashes, not raw token secrets.
- Normal clients should authenticate with device/session/API tokens, not storage credentials.
- Permission checks happen server-side for every metadata and transfer authorization operation.
- Impersonation requires explicit authorization and audit provenance.

## Metadata ownership

PostgreSQL is the server-side source of truth for metadata.

The server owns all writes to:

- orgs/users/devices/tokens/invites
- projects/worksets/workset rules
- namespace nodes
- file versions
- grants/shares/publishes
- snapshots
- locks
- conflicts
- operation log
- trash/retention
- audit events

Clients/daemons submit requests and operations. The server validates and applies them.

## Operation replay and offline reconciliation

The server treats client/offline operations as first-class records.

Rules:

- Daemons submit queued operations with client operation ID, device ID, actor, base node/version/snapshot IDs, params, source, and idempotency key.
- Server records the operation before applying, rejecting, or marking conflicted.
- Duplicate replays are idempotent.
- Server validates permissions, base state, locks, and current metadata before applying.
- If base state diverged, server creates conflict records and preserves all versions.
- Server never silently overwrites divergent work.

## Transfer authorization

Normal clients do not receive permanent object-storage credentials.

Transfer model:

1. Client/daemon requests upload/download authorization for a node/version/content manifest.
2. Server verifies actor, device, grant, lock/conflict state, and operation context.
3. Server issues short-lived scoped transfer authorization.
4. Client transfers content directly or through a server-mediated path depending on deployment policy.
5. Server verifies/records content manifest and commits file version after integrity checks.

Authorization may be implemented as:

- presigned object-storage URLs
- temporary scoped credentials
- server-mediated upload/download endpoints

The chosen implementation must preserve least privilege and auditability.

## Content and manifests

Object storage holds file data and content manifests. PostgreSQL stores file version metadata and manifest references.

Rules:

- Server commit requires content integrity verification.
- Manifest references must be immutable for committed versions.
- Orphaned content cleanup must be conservative and retention-aware.
- Normal clients should not be able to list arbitrary object-storage buckets.

## Snapshots and versions

The server owns snapshot metadata and restore semantics.

Rules:

- Snapshots can be org, project, workset, or subtree scoped.
- Snapshot restore is audited and data-moving.
- Restores copy/promote data without destroying current data by default.
- Snapshot retention/purge is admin/destructive policy behavior.

## Locks and conflicts

The server is authoritative for locks and conflict records.

Rules:

- Existing files lock by `node_id`.
- Offline-created files can reconcile provisional IDs to server node IDs.
- Lock acquisition/release/break is audited.
- Lock break is admin/destructive.
- Conflicts preserve all versions and link local/remote operations where possible.
- Conflict resolution is data-moving and audited.

## Audit

The server owns durable audit events.

Rules:

- Every meaningful server-applied operation should emit or link to an audit event.
- Audit event schema uses indexed envelope columns plus typed JSON payloads.
- Audit events must not contain secrets.
- Offline audit events can be accepted and acknowledged after replay.
- Audit export must support bounded/paginated/NDJSON output.

## Background worker responsibilities

Worker jobs may include:

- trash retention and purge
- snapshot creation/expiry
- invite expiry
- device/session cleanup
- orphan content detection/cleanup
- audit export/compaction jobs
- operation replay/reconciliation retries
- project/workset integration sync
- metrics/health rollups

Worker jobs must be idempotent and safe to retry.

## Migrations

Migrations are explicit and versioned.

Rules:

- The repo stores migrations in version order.
- Migration jobs/commands run as controlled release/deployment steps.
- Server startup must verify schema compatibility.
- Destructive migrations require extra review and backup/rollback notes.
- Self-hosted operators need clear migration instructions.

Current MVP foundation: `biohazardfs-server migrate` uses the resolved shared config (`[database].url` or `BIOHAZARDFS_DATABASE_URL`) to apply bundled Postgres migrations and records applied versions in `schema_migrations`. `/readyz` uses the same resolved config to verify migration compatibility when a database URL is configured. Both paths fail closed with redacted JSON error envelopes when the database URL is missing/unusable or migrations cannot be verified. The current synchronous Postgres client supports explicit plaintext only, so self-hosted/local URLs must include `sslmode=disable` until TLS support is implemented. The baseline creates only metadata tables/indexes; object-store/file APIs are intentionally out of scope.

## Docker packaging

The repo should include Docker packaging for the server.

Docker image requirements:

- non-root runtime user where practical
- minimal runtime image
- healthcheck or documented health endpoint
- explicit environment variables
- no baked-in secrets
- supports `serve`, `worker`, `migrate`, `health`, `version`, and redacted `config` modes
- logs to stdout/stderr in structured or parseable form

Expected environment variables include:

```text
BIOHAZARDFS_DATABASE_URL             # may also come from [database].url in config.toml
BIOHAZARDFS_OBJECT_STORE_PROVIDER   # default: rustfs
BIOHAZARDFS_OBJECT_STORE_ENDPOINT
BIOHAZARDFS_OBJECT_STORE_BUCKET
BIOHAZARDFS_OBJECT_STORE_REGION
BIOHAZARDFS_OBJECT_STORE_ACCESS_KEY_ID
BIOHAZARDFS_OBJECT_STORE_SECRET_ACCESS_KEY
BIOHAZARDFS_SERVER_PUBLIC_URL
BIOHAZARDFS_LOG
BIOHAZARDFS_CONFIG_FILE
```

Names may evolve, but config must be documented and secret-safe.

## Helm chart

The repo should include a Helm chart for self-hosted Kubernetes deployments.

Chart responsibilities:

- API Deployment
- Worker Deployment, optional/separate
- Migration Job/Hook
- Service
- Ingress, optional
- ConfigMap for non-secret config
- Secret references for database/object-storage credentials
- ServiceAccount/RBAC as needed
- probes/health checks
- resource requests/limits
- persistence only if needed by server runtime; object data lives in object storage

Chart should support external PostgreSQL and external S3-compatible object storage from the start. RustFS is the canonical BiohazardFS self-hosted object-store default, while other S3-compatible providers may be supported through the same contract. Bundled dev dependencies can be optional later, but production should not require in-chart databases/storage.

## Self-hosted deployment modes

Initial supported deployment modes should be:

1. Local/dev container compose-style deployment, later.
2. Kubernetes deployment through in-repo Helm chart.
3. Manual binary/container deployment for advanced operators.

Self-hosted docs must not depend on private Biohazard infrastructure.

## Hosted service boundary

The open-source server should be complete enough to self-host.

A future hosted BiohazardFS service may add private operational code around:

- billing
- tenant provisioning
- managed backups
- hosted observability
- support tooling
- cloud-specific infrastructure automation

But the core server/control plane required to run BiohazardFS must remain public and self-hostable.

## Observability

Server should expose:

- health endpoint
- readiness endpoint
- structured logs
- request IDs
- operation IDs
- audit event IDs
- basic metrics, later

Logs must not contain secrets.

## First implementation slice

The first server implementation should establish:

1. Server crate/binary skeleton.
2. Config loading and redacted config display through `biohazardfs-server config`.
3. Health/readiness endpoints.
4. Database connection and migration runner foundation.
5. Object-store config validation stub.
6. Standard JSON envelope and stable error codes.
7. Minimal auth/device token validation skeleton.
8. Metadata schema migration baseline.
9. Dockerfile that can run `serve`, `worker`, and `migrate` modes, even if worker is initially stubbed.
10. Helm chart skeleton with API deployment, worker deployment disabled or stubbed, migration job, service, config, secret references, and probes.

Do not implement client write support until the server can validate operations, enforce permissions, record audit events, and issue scoped transfer authorization safely.
