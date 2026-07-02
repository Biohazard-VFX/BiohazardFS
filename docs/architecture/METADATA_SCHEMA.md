# BiohazardFS Metadata Schema Contract

Status: draft reference
Audience: server/control-plane implementers, daemon implementers, CLI/agent authors, operators

The metadata schema is the server-side source of truth for identity, devices, namespace state, permissions, file versions, snapshots, locks, conflicts, offline reconciliation, trash, sharing, and audit provenance.

This document describes the logical schema. Physical table names and exact SQL migrations may differ, but implementations must preserve these identities, relationships, and invariants.

## Core decisions

- Include an org/studio boundary from day one.
- Model filesystem identity with stable `node_id`; path is derived from mutable `parent_node_id` + `name`.
- `FileVersion` points to a content manifest reference; v1 does not require server DB tables for individual chunks.
- Snapshots support org, project, workset, and subtree scopes.
- Grants can attach to projects, worksets, nodes, and shares.
- Locks attach to stable node IDs where possible, with path snapshots and provisional local IDs for offline-created files.
- Offline operations are first-class server records submitted by clients/daemons for replay/reconciliation.
- Deletes use trash records, soft-deleted nodes, and retention/purge policy.
- Audit events use a hybrid model: indexed envelope columns plus schema-versioned typed JSON payloads.

## ID conventions

IDs should be opaque, globally unique, and typed by prefix.

```text
org_       organization/studio
usr_       user
acct_      external/account identity if needed
dev_       enrolled device
tok_       API or device token
inv_       invite
proj_      project
wrk_       workset
node_      namespace node
ver_       file version
snap_      snapshot
lock_      lock
conf_      conflict
op_        client/offline/server operation
aud_       audit event
share_     share link/share grant
pub_       publish record
trash_     trash record
ret_       retention policy
obj_       content object/manifest logical ID
```

## Organization/studio boundary

Every primary record belongs to an `org_id` unless it is a global deployment/runtime record.

Logical fields:

```text
Organization
- org_id
- slug
- display_name
- status
- created_at
- updated_at
```

Rules:

- Users, devices, projects, worksets, nodes, snapshots, locks, conflicts, shares, and audit events are scoped by `org_id`.
- Queries must include org scoping by default.
- Cross-org access is out of scope for MVP.

## Users, identities, devices, and tokens

### User

```text
User
- org_id
- user_id
- display_name
- email
- role_hint
- status: active | disabled | invited
- created_at
- updated_at
```

### Device

```text
Device
- org_id
- device_id
- user_id
- display_name
- platform
- hostname
- public_key_ref, optional
- status: active | revoked | lost
- enrolled_at
- last_seen_at
- revoked_at
- revoked_by
```

### Token/session

```text
Token
- org_id
- token_id
- user_id
- device_id, optional
- kind: device | api | invite | service | local_exchange
- scopes
- status: active | revoked | expired
- issued_at
- expires_at
- revoked_at
- revoked_by
- secret_hash
```

Rules:

- Store token hashes, not raw token secrets.
- Device revocation invalidates device-scoped tokens and sessions.
- API/service tokens must be scoped and auditable.

### Invite

```text
Invite
- org_id
- invite_id
- created_by
- intended_email, optional
- default_project_id, optional
- default_workset_id, optional
- scopes/grants
- expires_at
- max_uses
- uses_count
- status: active | revoked | expired | exhausted
- created_at
- revoked_at
```

## Projects and worksets

### Project

```text
Project
- org_id
- project_id
- root_node_id
- name
- code
- status: active | archived
- created_at
- updated_at
```

A project root is a namespace node. Project folder structure must not be hardcoded; templates/integrations can create it.

### Workset

A workset is a curated/authorized subset of project or org namespace for users, devices, vendors, clients, agents, or assignments.

```text
Workset
- org_id
- workset_id
- project_id, optional
- name
- description
- status: active | archived
- source: manual | integration | invite | share | agent | server
- created_by
- created_at
- updated_at
```

### Workset membership/rules

```text
WorksetRule
- org_id
- workset_id
- rule_id
- kind: node | subtree | pattern | tag | integration_assignment
- node_id, optional
- pattern, optional
- permissions_hint
- created_at
```

Rules:

- Worksets do not duplicate files.
- Worksets determine visibility, default cache/pin intent, and access scope.
- Project tracker integrations may update worksets but are not required for BiohazardFS to function.

## Namespace nodes

The namespace is a stable node-ID tree.

```text
Node
- org_id
- node_id
- project_id, optional
- parent_node_id, null for org/project roots
- name
- kind: file | directory | symlink
- current_version_id, for files
- target, for symlink
- mode/permissions metadata
- owner_user_id, optional
- created_at
- created_by
- updated_at
- updated_by
- deleted_at
- deleted_by
- trash_id, optional
- path_cache, optional denormalized display path
```

Rules:

- `node_id` is stable across rename and move.
- Path is derived from parent/name. `path_cache` is advisory and rebuildable.
- Sibling names must be unique within a live parent, subject to platform/case-sensitivity policy.
- Deletes are soft by default and linked to trash records.
- Unauthorized nodes are hidden or denied according to access policy.
- Symlink behavior must be policy-controlled to avoid escaping authorized roots.

## File versions and content manifests

Every committed file write creates an immutable file version.

```text
FileVersion
- org_id
- version_id
- node_id
- parent_version_id, optional
- content_manifest_ref
- content_hash
- size_bytes
- logical_mtime
- created_at
- created_by
- created_device_id
- source: ui | cli | agent | api | server | test
- operation_id, optional
- audit_event_id, optional
- metadata_json
```

Rules:

- File versions are immutable.
- A node's `current_version_id` points to the current visible version.
- `content_manifest_ref` points to object storage metadata/manifest; individual chunks do not need first-class DB rows in v1.
- `content_hash` verifies complete file content or manifest identity according to storage implementation.
- Restores create a new version or promote a version through an audited operation; they do not mutate old versions.

## Content manifest reference

The metadata DB stores references to content manifests, not chunk rows.

Logical manifest requirements:

```text
ContentManifest
- logical object/manifest ID
- content hash
- size
- chunking strategy/version
- object storage references
- encryption/compression metadata, if used
```

This may live in object storage, server-side manifest storage, or an internal service. The DB only requires enough reference data to retrieve and verify content.

## Snapshots

Snapshots capture point-in-time state.

```text
Snapshot
- org_id
- snapshot_id
- scope_kind: org | project | workset | subtree
- scope_id, optional
- root_node_id, optional for subtree/project roots
- name
- description
- created_at
- created_by
- source: manual | schedule | preflight | agent | server
- retention_policy_id, optional
- state_ref
- status: creating | ready | failed | expired | purged
```

Rules:

- Snapshot scope supports org, project, workset, and subtree.
- Snapshots are read-only.
- Snapshot restore is data-moving and must be audited.
- Restores should copy/promote data without destroying current data by default.
- Snapshot state can be represented by a materialized tree reference, version map, storage snapshot reference, or equivalent implementation detail.

## Grants and permissions

Grants can attach to projects, worksets, nodes, and shares.

```text
Grant
- org_id
- grant_id
- subject_kind: user | group | device | token | invite | share | service
- subject_id
- resource_kind: project | workset | node | share
- resource_id
- permissions: hidden | read | write | admin | share | publish
- expires_at, optional
- constraints_json
- created_at
- created_by
- revoked_at
- revoked_by
```

Rules:

- Most users should receive access through project/workset grants.
- Node grants allow targeted overrides for folders/files.
- Share grants model external or client/vendor access.
- Permission widening is a stricter mutation under the CLI/daemon safety policy.
- Effective permissions must be queryable for a path/node/workset.

## Shares and publishes

### Share

```text
Share
- org_id
- share_id
- created_by
- resource_kind: node | workset | project | snapshot
- resource_id
- access_mode: read | write | review | download
- expires_at
- constraints_json
- status: active | revoked | expired
- created_at
- revoked_at
```

### Publish

```text
Publish
- org_id
- publish_id
- project_id, optional
- node_id
- version_id
- label
- comment
- created_by
- created_device_id
- source: ui | cli | agent | api | server
- status: active | superseded | revoked
- created_at
```

Rules:

- Publishing records an explicit version/provenance moment.
- Publishing may notify/update integrations but must remain valid without them.

## Locks

Locks protect files that should not be concurrently edited.

```text
Lock
- org_id
- lock_id
- node_id, optional
- provisional_local_id, optional
- path_snapshot
- owner_user_id
- owner_device_id
- kind: edit | admin | publish | restore
- status: active | released | expired | broken
- acquired_at
- expires_at
- released_at
- broken_at
- broken_by
- operation_id, optional
```

Rules:

- Existing files lock by `node_id`.
- `path_snapshot` is for display/audit and does not define identity when `node_id` exists.
- Offline-created files may use provisional local IDs until server node IDs are assigned.
- Lock break is admin/destructive and requires stricter safety.
- Stale lock handling must be explicit and audited.

## Conflicts

Conflicts preserve divergent versions and operations.

```text
Conflict
- org_id
- conflict_id
- node_id, optional
- path_snapshot
- kind: write_write | delete_write | rename_rename | rename_delete | permission | lock | other
- base_version_id, optional
- local_version_id, optional
- remote_version_id, optional
- local_operation_id, optional
- remote_operation_id, optional
- status: open | resolved | preserved_all | dismissed
- created_at
- resolved_at
- resolved_by
- resolution_json
```

Rules:

- Divergent reconnect always preserves both sides and creates conflict records.
- No silent overwrite.
- Automatic file-content merge is out of scope for MVP.
- Conflict resolution is data-moving and requires stricter safety in agent-safe mode.

## Offline/client operation log

Offline and optimistic operations are first-class server records.

```text
Operation
- org_id
- operation_id
- client_operation_id
- device_id
- actor_user_id
- impersonated_user_id, optional
- source: ui | cli | agent | api | server | test
- method
- params_json
- base_node_id, optional
- base_version_id, optional
- base_snapshot_id, optional
- idempotency_key
- status: received | accepted | applied | rejected | conflicted | superseded
- result_json
- conflict_id, optional
- created_at_client
- received_at_server
- applied_at_server
```

Rules:

- Daemons submit queued offline operations with base IDs/versions and idempotency keys.
- Server records each operation before applying or rejecting it.
- Replayed duplicate operations must be idempotent.
- If base state diverged, server creates conflicts instead of overwriting.
- Operation records link to audit events.

## Trash and retention

Deletes create trash entries and soft-delete nodes.

```text
TrashRecord
- org_id
- trash_id
- node_id
- original_parent_node_id
- original_name
- deleted_version_id, optional
- deleted_at
- deleted_by
- operation_id
- purge_after
- purged_at
- purged_by
- status: trashed | restored | purged
```

```text
RetentionPolicy
- org_id
- retention_policy_id
- name
- resource_kind: org | project | workset | node | snapshot | trash
- resource_id, optional
- rules_json
- created_at
- updated_at
```

Rules:

- Cloud/server delete is distinct from local cache removal.
- Trash restore is data-moving and audited.
- Purge is destructive/admin and requires stricter safety.
- Retention changes are admin/destructive policy mutations.

## Audit events

Audit events use indexed envelope columns plus schema-versioned typed JSON payloads.

```text
AuditEvent
- org_id
- audit_event_id
- event_type
- schema_version
- actor_user_id, optional
- impersonated_user_id, optional
- device_id, optional
- source: ui | cli | agent | api | server | test
- request_id, optional
- operation_id, optional
- project_id, optional
- workset_id, optional
- node_id, optional
- version_id, optional
- path_snapshot, optional
- result: success | failure | partial | queued
- created_at
- payload_json
```

Rules:

- Audit payload schemas are versioned and introspectable.
- Audit events must not contain secrets.
- Offline audit events can be locally recorded and later server-acknowledged.
- Audit queries must support path/node, actor, device, source, event type, and time range filters.
- Every meaningful file/version/permission/share/lock/conflict/snapshot operation should produce or link to an audit event.

## Invariants

- Every record is org-scoped unless explicitly global.
- Node identity is stable; paths are derived and mutable.
- File versions are immutable.
- Current visible file content is determined by `Node.current_version_id`.
- Deletes are soft until purged.
- Dirty/offline operations must be durable before being reported as accepted locally.
- Reconnect conflicts preserve all divergent data.
- Permission widening, purge, token/device revoke, snapshot rollback, conflict resolution, and restore/promote are stricter mutations.
- All meaningful operations are auditable with actor/device/source/request provenance.

## First implementation slice

The first server/control-plane schema implementation should include:

1. `Organization`, `User`, `Device`, `Token`, `Invite`.
2. `Project`, `Workset`, `WorksetRule`.
3. `Node` with stable IDs and soft delete fields.
4. `FileVersion` pointing to `content_manifest_ref`.
5. `Grant` for project/workset/node/share resources.
6. `Operation` for idempotent replay/reconciliation.
7. `AuditEvent` hybrid envelope/payload storage.
8. `Lock` and `Conflict` records.
9. `Snapshot` records with scope fields.
10. `TrashRecord` and basic retention policy.

Do not implement optimistic offline writes without operation idempotency, conflict creation, audit linkage, and dirty-data preservation tests.
