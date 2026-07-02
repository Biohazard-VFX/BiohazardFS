-- BiohazardFS MVP metadata baseline.
-- This migration only creates metadata tables and indexes. It does not create object-store
-- buckets, file APIs, seed data, or destructive database behavior.

CREATE TABLE organizations (
    org_id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    display_name TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE users (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    email TEXT,
    role_hint TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'invited')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (org_id, user_id)
);

CREATE UNIQUE INDEX users_org_email_unique
    ON users (org_id, lower(email))
    WHERE email IS NOT NULL;

CREATE TABLE tokens (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    token_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    device_id TEXT,
    kind TEXT NOT NULL CHECK (kind IN ('device', 'api', 'invite', 'service', 'local_exchange')),
    scopes JSONB NOT NULL DEFAULT '[]'::jsonb,
    status TEXT NOT NULL CHECK (status IN ('active', 'revoked', 'expired')),
    issued_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ,
    revoked_at TIMESTAMPTZ,
    revoked_by TEXT,
    secret_hash TEXT NOT NULL,
    PRIMARY KEY (org_id, token_id),
    FOREIGN KEY (org_id, user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX tokens_org_user_idx
    ON tokens (org_id, user_id);

CREATE TABLE nodes (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    node_id TEXT NOT NULL,
    project_id TEXT,
    parent_node_id TEXT,
    name TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('file', 'directory', 'symlink')),
    current_version_id TEXT,
    symlink_target TEXT,
    mode_bits INTEGER,
    owner_user_id TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_by TEXT,
    deleted_at TIMESTAMPTZ,
    deleted_by TEXT,
    trash_id TEXT,
    path_cache TEXT,
    PRIMARY KEY (org_id, node_id),
    FOREIGN KEY (org_id, parent_node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, owner_user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX nodes_live_sibling_name_unique
    ON nodes (org_id, COALESCE(parent_node_id, ''), lower(name))
    WHERE deleted_at IS NULL;

CREATE INDEX nodes_org_parent_idx
    ON nodes (org_id, parent_node_id)
    WHERE deleted_at IS NULL;

CREATE TABLE content_manifests (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    content_manifest_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    storage_provider TEXT NOT NULL,
    object_key TEXT NOT NULL,
    manifest_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT,
    PRIMARY KEY (org_id, content_manifest_id),
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX content_manifests_org_hash_idx
    ON content_manifests (org_id, content_hash);

CREATE TABLE file_versions (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    version_id TEXT NOT NULL,
    node_id TEXT NOT NULL,
    parent_version_id TEXT,
    content_manifest_id TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    size_bytes BIGINT NOT NULL CHECK (size_bytes >= 0),
    logical_mtime TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by TEXT,
    created_device_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('ui', 'cli', 'agent', 'api', 'server', 'test')),
    operation_id TEXT,
    audit_event_id TEXT,
    metadata_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (org_id, version_id),
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, parent_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, content_manifest_id) REFERENCES content_manifests(org_id, content_manifest_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT
);

CREATE INDEX file_versions_org_node_idx
    ON file_versions (org_id, node_id, created_at DESC);

CREATE TABLE operations (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    operation_id TEXT NOT NULL,
    actor_user_id TEXT,
    device_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('ui', 'cli', 'agent', 'api', 'server', 'test')),
    kind TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('pending', 'applying', 'applied', 'conflict', 'failed', 'cancelled')),
    base_version_id TEXT,
    node_id TEXT,
    path TEXT,
    idempotency_key TEXT,
    request_id TEXT,
    payload_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, operation_id),
    FOREIGN KEY (org_id, actor_user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, base_version_id) REFERENCES file_versions(org_id, version_id) ON DELETE RESTRICT
);

CREATE UNIQUE INDEX operations_org_idempotency_unique
    ON operations (org_id, idempotency_key)
    WHERE idempotency_key IS NOT NULL;

CREATE INDEX operations_org_status_idx
    ON operations (org_id, status, created_at);

CREATE TABLE upload_sessions (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    upload_session_id TEXT NOT NULL,
    node_id TEXT,
    operation_id TEXT,
    created_by TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'uploading', 'completed', 'aborted', 'expired')),
    content_manifest_id TEXT,
    size_bytes BIGINT CHECK (size_bytes IS NULL OR size_bytes >= 0),
    bytes_received BIGINT NOT NULL DEFAULT 0 CHECK (bytes_received >= 0),
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ,
    PRIMARY KEY (org_id, upload_session_id),
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, operation_id) REFERENCES operations(org_id, operation_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, created_by) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, content_manifest_id) REFERENCES content_manifests(org_id, content_manifest_id) ON DELETE RESTRICT
);

CREATE INDEX upload_sessions_org_status_idx
    ON upload_sessions (org_id, status, created_at);

CREATE TABLE audit_events (
    org_id TEXT NOT NULL REFERENCES organizations(org_id) ON DELETE RESTRICT,
    audit_event_id TEXT NOT NULL,
    actor_user_id TEXT,
    device_id TEXT,
    source TEXT NOT NULL CHECK (source IN ('ui', 'cli', 'agent', 'api', 'server', 'test')),
    event_type TEXT NOT NULL,
    node_id TEXT,
    operation_id TEXT,
    request_id TEXT,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    payload_schema_version TEXT NOT NULL DEFAULT '2026-07-audit-v1',
    payload_json JSONB NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (org_id, audit_event_id),
    FOREIGN KEY (org_id, actor_user_id) REFERENCES users(org_id, user_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, node_id) REFERENCES nodes(org_id, node_id) ON DELETE RESTRICT,
    FOREIGN KEY (org_id, operation_id) REFERENCES operations(org_id, operation_id) ON DELETE RESTRICT
);

CREATE INDEX audit_events_org_time_idx
    ON audit_events (org_id, occurred_at DESC);

CREATE INDEX audit_events_org_type_idx
    ON audit_events (org_id, event_type, occurred_at DESC);
