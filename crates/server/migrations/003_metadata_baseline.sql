-- BiohazardFS metadata baseline extension (Wave 2).
-- Net-new metadata tables for projects, worksets, devices, locks, conflicts,
-- snapshots, grants, shares, publishes, invites, trash, and retention policy.
--
-- This migration only creates tables and indexes. It does not migrate or
-- backfill data, it has no DOWN partner, and it does not ALTER existing tables
-- from 001_baseline (organizations, users, tokens, nodes, content_manifests,
-- file_versions, operations, upload_sessions, audit_events). Those tables are
-- referenced via FOREIGN KEYs from the new tables but are not modified here.
--
-- Field names mirror docs/architecture/METADATA_SCHEMA.md. Status values reuse
-- the lowercase text CHECK-convention from 001. Revocation/break/purge actors
-- (revoked_by, broken_by, purged_by, resolved_by) are intentionally loose
-- attribution columns without FKs, matching tokens.revoked_by in 001.

-- Enrolled devices. tokens.device_id (001) is a soft reference until this table
-- existed; device revocation invalidates device-scoped tokens and sessions.
CREATE TABLE devices (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    device_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    platform TEXT,
    hostname TEXT,
    public_key_ref TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'lost')),
    enrolled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    revoked_by TEXT,
    PRIMARY KEY (org_id, device_id),
    FOREIGN KEY (org_id, user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX devices_org_user_idx
    ON devices (org_id, user_id);

CREATE INDEX devices_org_status_idx
    ON devices (org_id, status, enrolled_at DESC);

-- Projects. A project root is a namespace node. nodes.project_id (001) remains
-- a soft reference; the project-root allocation contract is formalized later.
CREATE TABLE projects (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    project_id TEXT NOT NULL,
    root_node_id TEXT,
    name TEXT NOT NULL,
    code TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, project_id),
    FOREIGN KEY (org_id, root_node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX projects_org_code_unique
    ON projects (org_id, lower(code));

CREATE INDEX projects_org_status_idx
    ON projects (org_id, status, created_at DESC);

-- Worksets are curated subsets of project or org namespace. Worksets do not
-- duplicate files; they drive visibility and default cache/pin intent.
CREATE TABLE worksets (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    workset_id TEXT NOT NULL,
    project_id TEXT,
    name TEXT NOT NULL,
    description TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'archived')),
    source TEXT NOT NULL CHECK (source IN ('manual', 'integration', 'invite', 'share', 'agent', 'server')),
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, workset_id),
    FOREIGN KEY (org_id, project_id) REFERENCES projects(org_id, project_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX worksets_org_project_idx
    ON worksets (org_id, project_id, status);

CREATE INDEX worksets_org_status_idx
    ON worksets (org_id, status, created_at DESC);

-- Workset membership rules.
CREATE TABLE workset_rules (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    workset_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('node', 'subtree', 'pattern', 'tag', 'integration_assignment')),
    node_id TEXT,
    pattern TEXT,
    permissions_hint TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, workset_id, rule_id),
    FOREIGN KEY (org_id, workset_id) REFERENCES worksets(org_id, workset_id) ON DELETE CASCADE,
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT
);

CREATE INDEX workset_rules_org_workset_idx
    ON workset_rules (org_id, workset_id);

-- Retention policies. resource is polymorphic (discriminated by resource_kind),
-- so no FK is declared on resource_id. rules_json keeps policy mutation out of
-- the physical schema.
CREATE TABLE retention_policies (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    retention_policy_id TEXT NOT NULL,
    name TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('org', 'project', 'workset', 'node', 'snapshot', 'trash')),
    resource_id TEXT,
    rules_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, retention_policy_id)
);

CREATE INDEX retention_policies_org_resource_idx
    ON retention_policies (org_id, resource_kind, resource_id);

-- Snapshots capture point-in-time state at org/project/workset/subtree scope.
-- scope_id is polymorphic (discriminated by scope_kind), so no FK on scope_id.
CREATE TABLE snapshots (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    snapshot_id TEXT NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('org', 'project', 'workset', 'subtree')),
    scope_id TEXT,
    root_node_id TEXT,
    name TEXT NOT NULL,
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT,
    source TEXT NOT NULL CHECK (source IN ('manual', 'schedule', 'preflight', 'agent', 'server')),
    retention_policy_id TEXT,
    state_ref TEXT,
    status TEXT NOT NULL CHECK (status IN ('creating', 'ready', 'failed', 'expired', 'purged')),
    PRIMARY KEY (org_id, snapshot_id),
    FOREIGN KEY (org_id, root_node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, retention_policy_id) REFERENCES retention_policies(org_id, retention_policy_id) ON DELETE SET NULL
);

CREATE INDEX snapshots_org_scope_idx
    ON snapshots (org_id, scope_kind, scope_id);

CREATE INDEX snapshots_org_status_idx
    ON snapshots (org_id, status, created_at DESC);

-- Locks protect files from concurrent edit. node_id is optional so
-- offline-created files can lock against a provisional_local_id until the
-- server assigns a stable node_id during reconciliation.
CREATE TABLE locks (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    lock_id TEXT NOT NULL,
    node_id TEXT,
    provisional_local_id TEXT,
    path_snapshot TEXT,
    owner_user_id TEXT NOT NULL,
    owner_device_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('edit', 'admin', 'publish', 'restore')),
    status TEXT NOT NULL CHECK (status IN ('active', 'released', 'expired', 'broken')),
    acquired_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ,
    released_at TIMESTAMPTZ,
    broken_at TIMESTAMPTZ,
    broken_by TEXT,
    operation_id TEXT,
    PRIMARY KEY (org_id, lock_id),
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, owner_user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, owner_device_id) REFERENCES devices(org_id, device_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, operation_id) REFERENCES operations(org_id, operation_id) ON DELETE SET NULL
);

CREATE INDEX locks_org_node_idx
    ON locks (org_id, node_id)
    WHERE node_id IS NOT NULL;

CREATE INDEX locks_org_status_idx
    ON locks (org_id, status, acquired_at DESC);

-- Conflicts preserve divergent versions and operations. No silent overwrite:
-- reconnect reconciliation always records both sides here.
CREATE TABLE conflicts (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    conflict_id TEXT NOT NULL,
    node_id TEXT,
    path_snapshot TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('write_write', 'delete_write', 'rename_rename', 'rename_delete', 'permission', 'lock', 'other')),
    base_version_id TEXT,
    local_version_id TEXT,
    remote_version_id TEXT,
    local_operation_id TEXT,
    remote_operation_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('open', 'resolved', 'preserved_all', 'dismissed')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT,
    resolution_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (org_id, conflict_id),
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, base_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, local_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, remote_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, local_operation_id) REFERENCES operations(org_id, operation_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, remote_operation_id) REFERENCES operations(org_id, operation_id) ON DELETE SET NULL
);

CREATE INDEX conflicts_org_status_idx
    ON conflicts (org_id, status, created_at DESC);

CREATE INDEX conflicts_org_node_idx
    ON conflicts (org_id, node_id)
    WHERE node_id IS NOT NULL;

-- Grants attach permissions to projects/worksets/nodes/shares. subject and
-- resource are polymorphic (discriminated by subject_kind/resource_kind), so no
-- single FK is correct. permissions is a JSONB array of permission strings.
CREATE TABLE grants (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    grant_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL CHECK (subject_kind IN ('user', 'group', 'device', 'token', 'invite', 'share', 'service')),
    subject_id TEXT NOT NULL,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('project', 'workset', 'node', 'share')),
    resource_id TEXT NOT NULL,
    permissions JSONB NOT NULL DEFAULT '[]'::jsonb,
    expires_at TIMESTAMPTZ,
    constraints_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT,
    revoked_at TIMESTAMPTZ,
    revoked_by TEXT,
    PRIMARY KEY (org_id, grant_id),
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX grants_org_resource_idx
    ON grants (org_id, resource_kind, resource_id);

CREATE INDEX grants_org_subject_idx
    ON grants (org_id, subject_kind, subject_id);

-- Shares model external/client/vendor access to a resource. resource is
-- polymorphic (discriminated by resource_kind), so no FK on resource_id.
CREATE TABLE shares (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    share_id TEXT NOT NULL,
    created_by TEXT,
    resource_kind TEXT NOT NULL CHECK (resource_kind IN ('node', 'workset', 'project', 'snapshot')),
    resource_id TEXT NOT NULL,
    access_mode TEXT NOT NULL CHECK (access_mode IN ('read', 'write', 'review', 'download')),
    expires_at TIMESTAMPTZ,
    constraints_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, share_id),
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX shares_org_status_idx
    ON shares (org_id, status, created_at DESC);

-- Publishes record an explicit version/provenance moment.
CREATE TABLE publishes (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    publish_id TEXT NOT NULL,
    project_id TEXT,
    node_id TEXT NOT NULL,
    version_id TEXT NOT NULL,
    label TEXT,
    comment TEXT,
    created_by TEXT,
    created_device_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('ui', 'cli', 'agent', 'api', 'server')),
    status TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'revoked')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, publish_id),
    FOREIGN KEY (org_id, project_id) REFERENCES projects(org_id, project_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, version_id) REFERENCES file_versions(org_id, version_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_device_id) REFERENCES devices(org_id, device_id) ON DELETE SET NULL
);

CREATE INDEX publishes_org_node_idx
    ON publishes (org_id, node_id, created_at DESC);

-- Invites. scopes/grants are JSONB to keep invite mutation flexible.
CREATE TABLE invites (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    invite_id TEXT NOT NULL,
    created_by TEXT,
    intended_email TEXT,
    default_project_id TEXT,
    default_workset_id TEXT,
    scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
    grants JSONB NOT NULL DEFAULT '[]'::jsonb,
    expires_at TIMESTAMPTZ,
    max_uses INTEGER NOT NULL DEFAULT 1 CHECK (max_uses >= 0),
    uses_count INTEGER NOT NULL DEFAULT 0 CHECK (uses_count >= 0),
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired', 'exhausted')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, invite_id),
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, default_project_id) REFERENCES projects(org_id, project_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, default_workset_id) REFERENCES worksets(org_id, workset_id) ON DELETE SET NULL
);

CREATE INDEX invites_org_status_idx
    ON invites (org_id, status, created_at DESC);

-- Trash records. Cloud/server delete is distinct from local cache removal.
-- Purge is destructive, retention-aware, and audited.
CREATE TABLE trash_records (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    trash_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    original_parent_node_id TEXT,
    original_name TEXT NOT NULL,
    deleted_version_id TEXT,
    deleted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_by TEXT,
    operation_id TEXT,
    purge_after TIMESTAMPTZ,
    purged_at TIMESTAMPTZ,
    purged_by TEXT,
    status TEXT NOT NULL CHECK (status IN ('trashed', 'restored', 'purged')),
    PRIMARY KEY (org_id, trash_id),
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, original_parent_node_id) REFERENCES nodes(org_id, node_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, deleted_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE SET NULL,
    FOREIGN KEY (org_id, deleted_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, operation_id) REFERENCES operations(org_id, operation_id) ON DELETE SET NULL
);

CREATE INDEX trash_records_org_status_idx
    ON trash_records (org_id, status, deleted_at DESC);
