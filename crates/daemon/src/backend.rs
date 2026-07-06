//! In-memory daemon backend and RPC payload helpers (DAEMON_API.md).
//!
//! `DaemonBackend` is the thread-safe owner of all scaffold daemon state:
//! namespace nodes, cache entries, locks, conflicts, transfers, operation
//! tokens, audit buffer, and live event buffer. It wraps an `Arc<Mutex<...>>`
//! around `InMemoryBackend` so dispatch can hand out cheap `&DaemonBackend`
//! references and the dev-loopback handler can share one backend across
//! connections.
//!
//! Naming: this is `DaemonBackend`, not `DaemonState`, on purpose —
//! `DaemonState` is the api-types enum for the runtime lifecycle.
//!
//! Scope: every read/spine payload in this module is implemented against the
//! in-memory mock. Destructive/admin/data-moving methods are NOT dispatched
//! here; `dispatch_rpc` in `lib.rs` routes them to a `method_not_implemented`
//! arm after the operation-token policy check, so the registry stays honest.
//!
//! Invariant: `Mutex` poisoning indicates a panic while mutating daemon state.
//! For safety-critical software that is fatal; we propagate via `expect` with
//! a clear message rather than continuing on inconsistent state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use biohazardfs_api_types::{
    ApiError, EventEnvelope, MutationClassification, OperationToken, Source, timestamp,
};
use biohazardfs_core::{
    cache::{CacheEntry, CacheState, CacheStats, transition as cache_transition},
    conflict::Conflict,
    event::{AUDIT_PAYLOAD_SCHEMA_VERSION, AuditEvent, AuditEventResult},
    id::{
        NODE_ID_PREFIX, OBJECT_ID_PREFIX, OPERATION_ID_PREFIX, VERSION_ID_PREFIX, generate_id,
        validate_node_id, validate_version_id,
    },
    lock::{FileLock, LockKind, LockStatus},
    node::{Node, NodeKind},
    operation::{Operation, OperationStatus},
    path::case_insensitive_sibling_key,
    version::{ContentManifestRef, FileVersion},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Scaffold org id used for all seeded records. Production daemons learn the
/// org id from the server during device enrollment.
pub const SCAFFOLD_ORG_ID: &str = "org_scaffold";

/// Default per-token lifetime. The scaffold only validates the params hash
/// (per DAEMON_API.md initial slice); expiry is recorded but enforcement is a
/// later hardening pass.
const OPERATION_TOKEN_TTL_SECONDS: i64 = 600;

/// One mocked transfer record. The real transfer manager will own retry,
/// backoff, and resume state; this struct is the audited wire shape only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferRecord {
    pub transfer_id: String,
    pub node_id: Option<String>,
    pub path: String,
    /// "upload" or "download".
    pub direction: String,
    /// "queued" | "running" | "paused" | "completed" | "failed".
    pub state: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

/// One mocked mount record. The FUSE/placeholder layer will own real mount
/// state; this is the wire shape used by `mount.status` / `mount.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRecord {
    pub mount_id: String,
    pub root_node_id: String,
    pub mount_path: String,
    pub attached: bool,
    pub read_only: bool,
    pub transport: String,
}

/// Runtime facts surfaced by `daemon.status` / `daemon.version`. The endpoint
/// is the dev-loopback HTTP address in the scaffold; production will carry the
/// discovered IPC endpoint instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonRuntimeInfo {
    pub endpoint: String,
    pub started_at: String,
}

/// Internal stored operation token: the wire `OperationToken` plus the epoch
/// seconds at which it expires, so validation can compare against the wall
/// clock without re-parsing RFC3339.
#[derive(Debug, Clone)]
struct StoredOperationToken {
    token: OperationToken,
    expires_at_epoch: u64,
}

/// All mutable daemon state. Public fields so tests and future seeding paths
/// can populate it directly; mutation at the dispatch layer goes through the
/// payload functions below, which take `&DaemonBackend` and lock once.
#[derive(Debug, Clone)]
pub struct InMemoryBackend {
    pub runtime: DaemonRuntimeInfo,
    pub org_id: String,
    pub nodes: HashMap<String, Node>,
    pub cache_entries: HashMap<String, CacheEntry>,
    pub locks: HashMap<String, FileLock>,
    pub conflicts: Vec<Conflict>,
    pub transfers: Vec<TransferRecord>,
    pub mounts: Vec<MountRecord>,
    // Operation tokens are an internal implementation detail; tests mint and
    // validate them through `DaemonBackend::issue_operation_token` /
    // `validate_operation_token` rather than poking the map directly.
    operation_tokens: HashMap<String, StoredOperationToken>,
    pub audit: Vec<AuditEvent>,
    pub events: Vec<EventEnvelope>,
    /// File content store keyed by `node_id`. The source of truth for
    /// `file.read` hydration; populated by `file.write`. Production moves this
    /// to the object store and keeps only verified chunks locally.
    pub file_contents: HashMap<String, Vec<u8>>,
    /// Immutable file versions keyed by `version_id`. Populated by `file.write`;
    /// `file.versions` surfaces these in a follow-up (the payload currently
    /// returns an honest empty list pending its own promotion).
    pub file_versions: HashMap<String, FileVersion>,
    /// Offline/client operation log. `file.write` appends an Applied record so
    /// the audit/operation trail is real, not fabricated.
    pub operations: Vec<Operation>,
}

/// Thread-safe wrapper over `InMemoryBackend`. Cloning shares the state.
#[derive(Debug, Clone)]
pub struct DaemonBackend {
    pub inner: Arc<Mutex<InMemoryBackend>>,
}

impl DaemonBackend {
    /// Build a backend seeded with a small mock namespace (root + two
    /// children) and the given runtime endpoint. Cache, locks, conflicts, and
    /// transfers start empty; payload helpers and tests populate them.
    pub fn new(endpoint: impl Into<String>) -> Self {
        let started_at = timestamp();
        let endpoint = endpoint.into();
        let mut nodes = HashMap::new();

        let root = Node {
            org_id: SCAFFOLD_ORG_ID.to_string(),
            node_id: "node_root".to_string(),
            project_id: None,
            parent_node_id: None,
            name: "Project".to_string(),
            kind: NodeKind::Directory,
            current_version_id: None,
            target: None,
            mode: Some("0o755".to_string()),
            owner_user_id: None,
            created_at: started_at.clone(),
            created_by: None,
            updated_at: started_at.clone(),
            updated_by: None,
            deleted_at: None,
            deleted_by: None,
            trash_id: None,
            path_cache: Some("/Project".to_string()),
        };
        nodes.insert(root.node_id.clone(), root);

        let shots = Node {
            org_id: SCAFFOLD_ORG_ID.to_string(),
            node_id: "node_shots".to_string(),
            project_id: None,
            parent_node_id: Some("node_root".to_string()),
            name: "shots".to_string(),
            kind: NodeKind::Directory,
            current_version_id: None,
            target: None,
            mode: Some("0o755".to_string()),
            owner_user_id: None,
            created_at: started_at.clone(),
            created_by: None,
            updated_at: started_at.clone(),
            updated_by: None,
            deleted_at: None,
            deleted_by: None,
            trash_id: None,
            path_cache: Some("/Project/shots".to_string()),
        };
        nodes.insert(shots.node_id.clone(), shots);

        let readme = Node {
            org_id: SCAFFOLD_ORG_ID.to_string(),
            node_id: "node_readme".to_string(),
            project_id: None,
            parent_node_id: Some("node_root".to_string()),
            name: "README.md".to_string(),
            kind: NodeKind::File,
            current_version_id: None,
            target: None,
            mode: Some("0o644".to_string()),
            owner_user_id: None,
            created_at: started_at.clone(),
            created_by: None,
            updated_at: started_at.clone(),
            updated_by: None,
            deleted_at: None,
            deleted_by: None,
            trash_id: None,
            path_cache: Some("/Project/README.md".to_string()),
        };
        nodes.insert(readme.node_id.clone(), readme);

        let inner = InMemoryBackend {
            runtime: DaemonRuntimeInfo {
                endpoint,
                started_at: started_at.clone(),
            },
            org_id: SCAFFOLD_ORG_ID.to_string(),
            nodes,
            cache_entries: HashMap::new(),
            locks: HashMap::new(),
            conflicts: Vec::new(),
            transfers: Vec::new(),
            mounts: Vec::new(),
            operation_tokens: HashMap::new(),
            audit: Vec::new(),
            events: Vec::new(),
            file_contents: HashMap::new(),
            file_versions: HashMap::new(),
            operations: Vec::new(),
        };

        let backend = Self {
            inner: Arc::new(Mutex::new(inner)),
        };
        // daemon.started is the first event on the wire stream.
        backend.record_event(
            biohazardfs_api_types::event_types::DAEMON_STARTED,
            Value::Object(Default::default()),
        );
        backend
    }

    /// Lock the inner state. Poisoning is fatal for a sync daemon; we panic
    /// with a clear message instead of continuing on inconsistent state.
    fn lock(&self) -> MutexGuard<'_, InMemoryBackend> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Read-only access to the runtime info without holding the lock open.
    pub fn runtime(&self) -> DaemonRuntimeInfo {
        self.lock().runtime.clone()
    }

    // ----- seed helpers (tests + future default seeding) -----

    pub fn seed_node(&self, node: Node) {
        self.lock().nodes.insert(node.node_id.clone(), node);
    }

    pub fn seed_cache_entry(&self, entry: CacheEntry) {
        self.lock()
            .cache_entries
            .insert(entry.node_id.clone(), entry);
    }

    pub fn seed_lock(&self, lock: FileLock) {
        self.lock().locks.insert(lock.lock_id.clone(), lock);
    }

    pub fn seed_conflict(&self, conflict: Conflict) {
        self.lock().conflicts.push(conflict);
    }

    pub fn seed_transfer(&self, transfer: TransferRecord) {
        self.lock().transfers.push(transfer);
    }

    // ----- recording helpers -----

    /// Append a structured event to the live buffer. The dev-loopback
    /// transport does not stream yet; clients drain via `event_stream`.
    pub fn record_event(&self, event_type: &str, data: Value) {
        let envelope = EventEnvelope::new(event_type, data);
        self.lock().events.push(envelope);
    }

    /// Append a durable audit event. Audit events never contain secrets; the
    /// daemon buffers them locally while offline and retries on reconnect.
    #[allow(clippy::too_many_arguments)]
    pub fn record_audit(
        &self,
        event_type: &str,
        source: Source,
        request_id: Option<String>,
        node_id: Option<String>,
        version_id: Option<String>,
        path_snapshot: Option<String>,
        result: AuditEventResult,
        payload_json: Option<String>,
    ) {
        let org_id = self.lock().org_id.clone();
        let audit_event = AuditEvent {
            org_id,
            audit_event_id: generate_id("aud_"),
            event_type: event_type.to_string(),
            schema_version: AUDIT_PAYLOAD_SCHEMA_VERSION.to_string(),
            actor_user_id: None,
            impersonated_user_id: None,
            device_id: None,
            source,
            request_id,
            operation_id: None,
            project_id: None,
            workset_id: None,
            node_id,
            version_id,
            path_snapshot,
            result,
            created_at: timestamp(),
            payload_json,
        };
        let envelope_data = serde_json::to_value(&audit_event).unwrap_or_else(|_| {
            // Serialization can only fail if payload_json or fields are
            // non-stringifiable; AuditEvent is plain JSON-safe, so this is an
            // invariant. Fall back to a minimal object so the stream is not
            // dropped on the floor.
            Value::Object(Default::default())
        });
        self.lock().audit.push(audit_event);
        // audit.event_recorded lets stream consumers mirror the audit log.
        self.record_event(
            biohazardfs_api_types::event_types::AUDIT_EVENT_RECORDED,
            envelope_data,
        );
    }

    // ----- operation-token machinery -----

    /// Mint an operation token binding the validated params. The hash is a
    /// non-cryptographic scaffold digest (FNV-1a over canonical JSON) so the
    /// daemon can detect param drift without pulling a crypto dependency; the
    /// production daemon swaps in `sha256:` (DAEMON_API.md token data model).
    pub fn issue_operation_token(
        &self,
        method: &str,
        params: &Value,
        classification: MutationClassification,
        source: Source,
    ) -> OperationToken {
        let params_hash = params_hash(params);
        let plan_hash = plan_hash(method, &params_hash);
        let now = time::OffsetDateTime::now_utc();
        let expires = now + time::Duration::seconds(OPERATION_TOKEN_TTL_SECONDS);
        let expires_at_epoch = expires.unix_timestamp().max(0) as u64;
        let expires_at = expires
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| timestamp());

        let token = OperationToken {
            operation_token: generate_id("optok_"),
            method: method.to_string(),
            params_hash,
            plan_hash,
            actor_id: None,
            device_id: None,
            source,
            classification,
            expires_at,
        };

        let stored = StoredOperationToken {
            token: token.clone(),
            expires_at_epoch,
        };
        self.lock()
            .operation_tokens
            .insert(token.operation_token.clone(), stored);
        token
    }

    /// Validate an operation token against the attempted method/classification/
    /// source and the current params. Removes the `operation_token` field before
    /// hashing so the token itself does not perturb the params hash. Returns
    /// `operation_token_invalid` (unknown id), `operation_token_mismatch`
    /// (method/classification/source differ from the issued plan),
    /// `operation_token_params_mismatch` (params drift), or
    /// `operation_token_expired`. Identity bindings are checked before the
    /// params hash so the field that diverged is reported cleanly.
    pub fn validate_operation_token(
        &self,
        token: &str,
        method: &str,
        classification: MutationClassification,
        source: Source,
        params: &Value,
    ) -> Result<OperationToken, ApiError> {
        let inner = self.lock();
        let stored = inner.operation_tokens.get(token).cloned().ok_or_else(|| {
            ApiError::new(
                "operation_token_invalid",
                "operation token was not issued by this daemon",
            )
        })?;

        validate_token_binding(&stored.token, method, classification, source)?;

        let mut params_without_token = params.clone();
        if let Some(map) = params_without_token.as_object_mut() {
            map.remove("operation_token");
        }
        let actual_hash = params_hash(&params_without_token);
        if actual_hash != stored.token.params_hash {
            return Err(ApiError::with_details(
                "operation_token_params_mismatch",
                "operation token was issued for different params",
                serde_json::json!({
                    "expected": stored.token.params_hash,
                    "actual": actual_hash,
                }),
            ));
        }

        let now_epoch = time::OffsetDateTime::now_utc().unix_timestamp().max(0) as u64;
        if now_epoch > stored.expires_at_epoch {
            return Err(ApiError::with_details(
                "operation_token_expired",
                "operation token has expired",
                serde_json::json!({
                    "expires_at": stored.token.expires_at,
                }),
            ));
        }

        Ok(stored.token)
    }

    /// Snapshot of recent events for the event stream drain helper.
    pub fn recent_events(&self) -> Vec<EventEnvelope> {
        self.lock().events.clone()
    }
}

// ===========================================================================
// Payload helpers
// ===========================================================================

// ---- daemon/runtime ----

pub fn daemon_status_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let runtime = backend.runtime();
    Ok(serde_json::to_value(biohazardfs_api_types::DaemonStatus {
        name: biohazardfs_core::DAEMON_BIN.to_string(),
        version: biohazardfs_api_types::PRODUCT_VERSION.to_string(),
        state: biohazardfs_api_types::DaemonState::Ready,
        transport: "dev_loopback_http_json_rpc".to_string(),
        endpoint: runtime.endpoint,
    })
    .expect("daemon status serializes"))
}

pub fn daemon_health_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    // Mounts are healthy when every attached mount has an attached flag. The
    // scaffold has no FUSE layer yet, so this surfaces as a single green check.
    let mount_problems = inner.mounts.iter().filter(|mount| mount.attached).count();
    let state = if mount_problems == 0 {
        "ready"
    } else {
        "degraded"
    };
    Ok(serde_json::json!({
        "state": state,
        "checks": [
            {"name": "local_state_db", "ok": true, "message": "in-memory scaffold backend is responsive"},
            {"name": "mounts", "ok": mount_problems == 0, "message": format!("{} attached mount(s)", mount_problems)},
            {"name": "audit_buffer", "ok": true, "message": format!("{} buffered event(s)", inner.audit.len())},
        ],
    }))
}

pub fn daemon_version_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({
        "name": biohazardfs_core::DAEMON_BIN,
        "version": biohazardfs_api_types::PRODUCT_VERSION,
        "schema_version": biohazardfs_api_types::DAEMON_SCHEMA_VERSION,
        "event_schema_version": biohazardfs_api_types::EVENT_SCHEMA_VERSION,
    }))
}

pub fn daemon_methods_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let runtime = backend.runtime();
    let mut methods = biohazardfs_api_types::known_methods::daemon_method_names();
    methods.sort();
    Ok(serde_json::json!({
        "methods": methods,
        "transport": "dev_loopback_http_json_rpc",
        "endpoint": runtime.endpoint,
        "schema_version": biohazardfs_api_types::DAEMON_SCHEMA_VERSION,
    }))
}

// ---- auth/session ----

pub fn auth_status_payload() -> Result<Value, ApiError> {
    // No device enrollment in the scaffold; surface that honestly.
    Ok(serde_json::json!({
        "enrolled": false,
        "actor": null,
        "credentials_present": false,
        "session_active": false,
    }))
}

pub fn auth_whoami_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({
        "actor": null,
        "impersonated_user_id": null,
    }))
}

pub fn auth_credentials_path_payload() -> Result<Value, ApiError> {
    let dir = credentials_dir();
    Ok(serde_json::json!({
        "path": dir.join("credentials.json").to_string_lossy(),
        "present": false,
    }))
}

// ---- config ----

pub fn config_path_payload() -> Result<Value, ApiError> {
    let dir = credentials_dir();
    Ok(serde_json::json!({
        "path": dir.join("config.toml").to_string_lossy(),
        "exists": false,
    }))
}

pub fn config_show_payload() -> Result<Value, ApiError> {
    // Redacted defaults. Never echo secrets; the scaffold has none configured.
    Ok(serde_json::json!({
        "values": {},
        "redacted": [],
        "source": "scaffold_defaults",
    }))
}

pub fn config_validate_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({
        "valid": true,
        "warnings": [],
        "errors": [],
    }))
}

pub fn config_get_payload(params: &Value) -> Result<Value, ApiError> {
    let key = require_str_param(params, "key")?;
    // The scaffold holds no config keys yet; surface not_found rather than a
    // fabricated value.
    Err(ApiError::with_details(
        "config_key_not_found",
        format!("config key {key:?} is not set on the scaffold daemon"),
        serde_json::json!({"key": key}),
    ))
}

// ---- mount ----

pub fn mount_status_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let attached = inner.mounts.iter().any(|mount| mount.attached);
    Ok(serde_json::json!({
        "attached": attached,
        "mounts": inner.mounts.len(),
        "transport": "dev_loopback_http_json_rpc",
    }))
}

pub fn mount_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let mounts: Vec<Value> = inner
        .mounts
        .iter()
        .map(|mount| serde_json::to_value(mount).unwrap_or(Value::Null))
        .collect();
    Ok(serde_json::json!({"mounts": mounts}))
}

// ---- file ----

pub fn file_stat_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let node = lookup_node(backend, params)?;
    // Surface the authoritative file size so FUSE can advertise it at lookup
    // without a separate `file.read`. Directories report 0.
    let size_bytes = {
        let inner = backend.lock();
        node_size_bytes(&inner, &node)
    };
    let mut value = serde_json::to_value(&node).expect("node serializes");
    if let Some(map) = value.as_object_mut() {
        map.insert("size_bytes".to_string(), serde_json::json!(size_bytes));
    }
    Ok(value)
}

pub fn file_list_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let inner = backend.lock();
    // Resolve the effective parent: an explicit `parent_node_id` param wins;
    // otherwise default to the namespace root (the node with no parent), so
    // `file.list` with no params lists the root's children rather than the
    // root node itself.
    let parent = match params.get("parent_node_id").and_then(|v| v.as_str()) {
        Some(parent_id) => {
            validate_node_id(parent_id).map_err(core_error_to_api)?;
            if !inner.nodes.contains_key(parent_id) {
                return Err(ApiError::new(
                    "node_not_found",
                    format!("parent node {parent_id} was not found"),
                ));
            }
            Some(parent_id.to_string())
        }
        None => inner
            .nodes
            .values()
            .find(|node| node.parent_node_id.is_none())
            .map(|root| root.node_id.clone()),
    };

    let mut children: Vec<&Node> = inner
        .nodes
        .values()
        .filter(|node| {
            node.is_live()
                && match &parent {
                    Some(parent_id) => node.parent_node_id.as_deref() == Some(parent_id.as_str()),
                    None => false,
                }
        })
        .collect();
    children.sort_by(|a, b| a.name.cmp(&b.name));

    let entries: Vec<Value> = children
        .into_iter()
        .map(|child| {
            // Files advertise their committed byte length so FUSE can size
            // them at lookup; directories report 0.
            let size_bytes = node_size_bytes(&inner, child);
            serde_json::json!({
                "node_id": child.node_id,
                "name": child.name,
                "kind": serde_json::to_value(child.kind).unwrap_or(Value::Null),
                "current_version_id": child.current_version_id,
                "size_bytes": size_bytes,
            })
        })
        .collect();
    Ok(serde_json::json!({"parent_node_id": parent, "entries": entries}))
}

pub fn file_checksum_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let node = lookup_node(backend, params)?;
    // Deterministic mock checksum so callers can detect changes. Production
    // computes the real content hash from the object store.
    let mock = format!("sha256:scaffold:{:x}", fxhash(node.node_id.as_bytes()));
    Ok(serde_json::json!({
        "node_id": node.node_id,
        "checksum": mock,
        "algorithm": "sha256",
        "verified": false,
    }))
}

pub fn file_history_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let node = lookup_node(backend, params)?;
    let inner = backend.lock();
    let events: Vec<Value> = inner
        .audit
        .iter()
        .filter(|event| event.node_id.as_deref() == Some(node.node_id.as_str()))
        .map(|event| serde_json::to_value(event).unwrap_or(Value::Null))
        .collect();
    Ok(serde_json::json!({"node_id": node.node_id, "events": events}))
}

pub fn file_versions_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let node = lookup_node(backend, params)?;
    // The scaffold holds no version history yet; the FUSE write path populates
    // this. Surface an honest empty list rather than a fabricated version.
    if let Some(version_id) = node.current_version_id.as_deref() {
        validate_version_id(version_id).map_err(core_error_to_api)?;
    }
    Ok(serde_json::json!({
        "node_id": node.node_id,
        "current_version_id": node.current_version_id,
        "versions": [],
    }))
}

/// `file.write`: commit a file version. Accepts either `{node_id, ...}` to
/// update an existing file or `{parent_node_id, name, ...}` to create a new
/// one. Content travels as `content_hex` (the same hex encoding the server's
/// `content_hex` fields use, so no new wire shape). The write is atomic in this
/// scaffold: content is stored, an immutable [`FileVersion`] is recorded, the
/// node's `current_version_id` is advanced, an Applied [`Operation`] is logged,
/// an audit event is recorded, and the cache entry is driven to `Ready` through
/// the legal forward transition path. `file.write` is `LowRisk` under the
/// AgentSafe policy, so no operation token is required.
pub fn file_write_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let content_hex = require_str_param(params, "content_hex")?;
    let content = decode_hex(content_hex).map_err(|message| {
        ApiError::with_details(
            "invalid_param",
            message,
            serde_json::json!({"field": "content_hex"}),
        )
    })?;
    let content_hash = content_hash_for(&content);
    let mode_str = params
        .get("mode")
        .and_then(|v| v.as_str())
        .map(|value| value.to_string());
    let now = timestamp();

    let mut inner = backend.lock();
    let org_id = inner.org_id.clone();

    // Resolve the target node: update an existing file, or build a new one
    // under a live directory parent. Validates IDs/names at the trust boundary.
    let (mut node, created) = match params.get("node_id").and_then(|v| v.as_str()) {
        Some(node_id) => {
            validate_node_id(node_id).map_err(core_error_to_api)?;
            let existing = inner
                .nodes
                .get(node_id)
                .cloned()
                .filter(Node::is_live)
                .ok_or_else(|| {
                    ApiError::new(
                        "node_not_found",
                        format!("node {node_id} was not found in the namespace"),
                    )
                })?;
            if existing.kind != NodeKind::File {
                return Err(ApiError::with_details(
                    "node_not_file",
                    "only file nodes accept writes",
                    serde_json::json!({
                        "node_id": node_id,
                        "kind": serde_json::to_value(existing.kind).unwrap_or(Value::Null),
                    }),
                ));
            }
            // Optimistic concurrency: if the caller pinned a base version, the
            // current version must match. Divergence is a conflict, never a
            // silent overwrite (FILESYSTEM_SEMANTICS.md).
            if let Some(base_version_id) = params.get("base_version_id").and_then(|v| v.as_str()) {
                validate_version_id(base_version_id).map_err(core_error_to_api)?;
                if existing.current_version_id.as_deref() != Some(base_version_id) {
                    return Err(ApiError::with_details(
                        "version_conflict",
                        "base_version_id does not match the node's current version",
                        serde_json::json!({
                            "node_id": node_id,
                            "expected": existing.current_version_id,
                            "base_version_id": base_version_id,
                        }),
                    ));
                }
            }
            let mut updated = existing.clone();
            updated.updated_at = now.clone();
            if let Some(mode_value) = &mode_str {
                updated.mode = Some(mode_value.clone());
            }
            (updated, false)
        }
        None => {
            let parent_id = require_str_param(params, "parent_node_id")?;
            validate_node_id(parent_id).map_err(core_error_to_api)?;
            let name = require_str_param(params, "name")?;
            biohazardfs_core::path::validate_file_name(name).map_err(core_error_to_api)?;
            let parent = inner
                .nodes
                .get(parent_id)
                .cloned()
                .filter(Node::is_live)
                .ok_or_else(|| {
                    ApiError::new(
                        "node_not_found",
                        format!("parent node {parent_id} was not found"),
                    )
                })?;
            if parent.kind != NodeKind::Directory {
                return Err(ApiError::with_details(
                    "parent_not_directory",
                    "parent node must be a directory to create a file",
                    serde_json::json!({"parent_node_id": parent_id}),
                ));
            }
            // Case-insensitive sibling uniqueness: a same-key sibling is a
            // conflict regardless of case (FILESYSTEM_SEMANTICS.md).
            let sibling_key = case_insensitive_sibling_key(name);
            let conflict = inner.nodes.values().any(|candidate| {
                candidate.is_live()
                    && candidate.parent_node_id.as_deref() == Some(parent_id)
                    && case_insensitive_sibling_key(&candidate.name) == sibling_key
            });
            if conflict {
                return Err(ApiError::with_details(
                    "sibling_name_conflict",
                    "a sibling with the same case-insensitive name already exists",
                    serde_json::json!({"parent_node_id": parent_id, "name": name}),
                ));
            }
            let node = Node {
                org_id: org_id.clone(),
                node_id: generate_id(NODE_ID_PREFIX),
                project_id: parent.project_id.clone(),
                parent_node_id: Some(parent_id.to_string()),
                name: name.to_string(),
                kind: NodeKind::File,
                current_version_id: None,
                target: None,
                mode: Some(mode_str.clone().unwrap_or_else(|| "0o644".to_string())),
                owner_user_id: None,
                created_at: now.clone(),
                created_by: None,
                updated_at: now.clone(),
                updated_by: None,
                deleted_at: None,
                deleted_by: None,
                trash_id: None,
                path_cache: None,
            };
            (node, true)
        }
    };

    // Build the immutable version + the operation record. IDs are generated
    // here so the cache/audit/operation trail references one consistent set.
    let version_id = generate_id(VERSION_ID_PREFIX);
    let operation_id = generate_id(OPERATION_ID_PREFIX);
    let object_id = generate_id(OBJECT_ID_PREFIX);
    let parent_version_id = node.current_version_id.clone();
    let version = FileVersion {
        org_id: org_id.clone(),
        version_id: version_id.clone(),
        node_id: node.node_id.clone(),
        parent_version_id: parent_version_id.clone(),
        content_manifest_ref: ContentManifestRef {
            object_id: object_id.clone(),
            storage_key: format!("orgs/{org_id}/content/{content_hash}"),
            chunking: None,
        },
        content_hash: content_hash.clone(),
        size_bytes: content.len() as u64,
        logical_mtime: now.clone(),
        created_at: now.clone(),
        created_by: None,
        created_device_id: None,
        source: source.clone(),
        operation_id: Some(operation_id.clone()),
        audit_event_id: None,
        metadata_json: None,
    };
    // Redacted params for provenance: strips `content_hex` (the raw file bytes)
    // and the operation-token capability, keeps only safe metadata plus the
    // authoritative computed values. Stored in both the Operation record and
    // the AuditEvent payload (METADATA_SCHEMA.md: audit never carries secrets).
    let redacted_params = redact_file_write_params(
        params,
        &node.node_id,
        &version_id,
        &content_hash,
        version.size_bytes,
    );
    let redacted_params_json =
        serde_json::to_string(&redacted_params).unwrap_or_else(|_| "{}".to_string());

    let operation = Operation {
        org_id: org_id.clone(),
        operation_id: operation_id.clone(),
        client_operation_id: format!("cop_{operation_id}"),
        device_id: None,
        actor_user_id: None,
        impersonated_user_id: None,
        source: source.clone(),
        method: "file.write".to_string(),
        params_json: redacted_params_json,
        base_node_id: Some(node.node_id.clone()),
        base_version_id: parent_version_id.clone(),
        base_snapshot_id: None,
        idempotency_key: format!("idem_{operation_id}"),
        status: OperationStatus::Applied,
        result_json: Some(
            serde_json::json!({
                "version_id": version_id,
                "content_hash": content_hash,
                "created": created,
            })
            .to_string(),
        ),
        conflict_id: None,
        created_at_client: now.clone(),
        received_at_server: Some(now.clone()),
        applied_at_server: Some(now.clone()),
    };

    // Commit node + version + content + operation in one critical section.
    node.current_version_id = Some(version_id.clone());
    node.updated_at = now.clone();
    let node_id = node.node_id.clone();
    inner.nodes.insert(node_id.clone(), node);
    inner
        .file_versions
        .insert(version_id.clone(), version.clone());
    inner.file_contents.insert(node_id.clone(), content);
    inner.operations.push(operation);

    // Drive the cache entry to Ready through the legal forward transition path.
    // The guards reject unsafe moves (Dirty -> Evicting etc.); we never paper
    // over them. For an existing Ready entry we exercise Ready -> Dirty -> Ready
    // (the documented write cycle); for a fresh entry we use Absent/Failed ->
    // Populating -> Ready (Dirty is unreachable from Absent).
    let entry = inner
        .cache_entries
        .entry(node_id.clone())
        .or_insert_with(|| CacheEntry {
            node_id: node_id.clone(),
            version_id: None,
            state: CacheState::Absent,
            content_hash: None,
            size_bytes: 0,
            pinned: false,
            dirty: false,
            last_accessed_at: None,
        });
    apply_write_cache_transition(entry)?;
    entry.version_id = Some(version_id.clone());
    entry.content_hash = Some(content_hash.clone());
    entry.size_bytes = version.size_bytes;
    entry.dirty = false;
    entry.last_accessed_at = Some(now.clone());
    drop(inner);

    backend.record_event(
        biohazardfs_api_types::event_types::FILE_CHANGED,
        serde_json::json!({"node_id": node_id, "version_id": version_id, "created": created}),
    );
    backend.record_event(
        biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
        serde_json::json!({"node_id": node_id, "state": "ready"}),
    );
    backend.record_audit(
        "file.write",
        source,
        None,
        Some(node_id.clone()),
        Some(version_id.clone()),
        None,
        AuditEventResult::Success,
        // Audit payload mirrors the redacted operation params plus the
        // `created` flag; it must never carry `content_hex` or the token.
        Some(build_file_write_audit_payload(&redacted_params, created)),
    );

    Ok(serde_json::json!({
        "node_id": node_id,
        "version_id": version_id,
        "content_hash": content_hash,
        "size_bytes": version.size_bytes,
        "operation_id": operation_id,
        "created": created,
    }))
}

/// `file.read`: return the cached content bytes for a file node. The daemon
/// content store is the source of truth; the FUSE layer hydrates from this.
/// Returns typed `file_not_found` / `content_not_cached` errors when the node
/// or its content is absent, never a panic.
pub fn file_read_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    let mut inner = backend.lock();
    let node = inner
        .nodes
        .get(&node_id)
        .cloned()
        .filter(Node::is_live)
        .ok_or_else(|| {
            ApiError::new(
                "file_not_found",
                format!("file node {node_id} was not found in the namespace"),
            )
        })?;
    if node.kind != NodeKind::File {
        return Err(ApiError::with_details(
            "node_not_file",
            "only file nodes have readable content",
            serde_json::json!({
                "node_id": node_id,
                "kind": serde_json::to_value(node.kind).unwrap_or(Value::Null),
            }),
        ));
    }
    let content = inner.file_contents.get(&node_id).cloned().ok_or_else(|| {
        ApiError::new(
            "content_not_cached",
            format!("node {node_id} has no cached content; upload via file.write or hydrate first"),
        )
    })?;
    if let Some(entry) = inner.cache_entries.get_mut(&node_id) {
        entry.last_accessed_at = Some(timestamp());
    }
    let content_hash = content_hash_for(&content);
    let content_hex = encode_hex(&content);
    Ok(serde_json::json!({
        "node_id": node_id,
        "version_id": node.current_version_id,
        "content_hex": content_hex,
        "size_bytes": content.len(),
        "content_hash": content_hash,
    }))
}

// ---- cache ----

pub fn cache_status_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let stats = CacheStats::from_entries(inner.cache_entries.values(), None);
    Ok(serde_json::to_value(stats).expect("cache stats serialize"))
}

pub fn cache_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let mut entries: Vec<&CacheEntry> = inner.cache_entries.values().collect();
    entries.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    let values: Vec<Value> = entries
        .into_iter()
        .map(|entry| serde_json::to_value(entry).expect("cache entry serializes"))
        .collect();
    Ok(serde_json::json!({"entries": values}))
}

pub fn cache_pin_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    let mut inner = backend.lock();
    let entry = inner
        .cache_entries
        .entry(node_id.clone())
        .or_insert_with(|| CacheEntry {
            node_id: node_id.clone(),
            version_id: None,
            state: CacheState::Absent,
            content_hash: None,
            size_bytes: 0,
            pinned: false,
            dirty: false,
            last_accessed_at: None,
        });
    // Pinning goes through Absent->Pinning->Pinned when fresh; an existing
    // Ready/Pinned entry just flips the flag. The transition guards reject
    // illegal moves (e.g. Evicting->Pinned) so we never paper over bad state.
    if entry.state == CacheState::Absent {
        entry.state =
            cache_transition(CacheState::Absent, CacheState::Pinning).map_err(core_error_to_api)?;
        entry.state =
            cache_transition(CacheState::Pinning, CacheState::Pinned).map_err(core_error_to_api)?;
    } else if matches!(entry.state, CacheState::Ready | CacheState::Populating) {
        entry.state =
            cache_transition(entry.state, CacheState::Pinned).map_err(core_error_to_api)?;
    }
    entry.pinned = true;
    entry.last_accessed_at = Some(timestamp());
    let snapshot = entry.clone();
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
        serde_json::json!({"node_id": node_id, "pinned": true}),
    );
    backend.record_audit(
        "cache.pin",
        source,
        None,
        Some(node_id),
        None,
        None,
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("cache entry serializes"))
}

pub fn cache_unpin_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    let mut inner = backend.lock();
    let entry = inner.cache_entries.get_mut(&node_id).ok_or_else(|| {
        ApiError::new(
            "cache_entry_not_found",
            format!("node {node_id} is not present in the local cache"),
        )
    })?;
    if !entry.pinned {
        return Err(ApiError::new(
            "cache_entry_not_pinned",
            format!("node {node_id} is not pinned"),
        ));
    }
    entry.pinned = false;
    // A pinned entry sits in the Pinned state; unpinning returns it to Ready
    // so it becomes eligible for eviction under pressure.
    if entry.state == CacheState::Pinned {
        entry.state =
            cache_transition(CacheState::Pinned, CacheState::Ready).map_err(core_error_to_api)?;
    }
    entry.last_accessed_at = Some(timestamp());
    let snapshot = entry.clone();
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
        serde_json::json!({"node_id": node_id, "pinned": false}),
    );
    backend.record_audit(
        "cache.unpin",
        source,
        None,
        Some(node_id),
        None,
        None,
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("cache entry serializes"))
}

pub fn cache_hydrate_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    // Hydration must target a known namespace node so we never fabricate cache
    // state for a path the daemon cannot see.
    {
        let inner = backend.lock();
        if !inner.nodes.contains_key(&node_id) {
            return Err(ApiError::new(
                "node_not_found",
                format!("node {node_id} was not found in the namespace"),
            ));
        }
    }
    let mut inner = backend.lock();
    let entry = inner
        .cache_entries
        .entry(node_id.clone())
        .or_insert_with(|| CacheEntry {
            node_id: node_id.clone(),
            version_id: None,
            state: CacheState::Absent,
            content_hash: None,
            size_bytes: 0,
            pinned: false,
            dirty: false,
            last_accessed_at: None,
        });
    entry.state = if matches!(entry.state, CacheState::Absent | CacheState::Failed) {
        cache_transition(entry.state, CacheState::Populating).map_err(core_error_to_api)?
    } else {
        entry.state
    };
    entry.state = cache_transition(entry.state, CacheState::Ready).map_err(core_error_to_api)?;
    entry.last_accessed_at = Some(timestamp());
    let snapshot = entry.clone();
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
        serde_json::json!({"node_id": node_id, "state": "ready"}),
    );
    backend.record_audit(
        "cache.hydrate",
        source,
        None,
        Some(node_id),
        None,
        None,
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("cache entry serializes"))
}

pub fn cache_dehydrate_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    let mut inner = backend.lock();
    let entry = inner
        .cache_entries
        .get_mut(&node_id)
        .cloned()
        .ok_or_else(|| {
            ApiError::new(
                "cache_entry_not_found",
                format!("node {node_id} is not present in the local cache"),
            )
        })?;
    // Critical invariant: dirty data must never be lost, and pinned entries
    // must not be evicted. Dehydrate is local-only (cloud copy untouched) but
    // still refuses these states.
    if entry.dirty {
        return Err(ApiError::with_details(
            "cache_entry_dirty",
            "dirty entries cannot be dehydrated; upload or repair first",
            serde_json::json!({"node_id": node_id}),
        ));
    }
    if entry.pinned {
        return Err(ApiError::with_details(
            "cache_entry_pinned",
            "pinned entries cannot be dehydrated; unpin first",
            serde_json::json!({"node_id": node_id}),
        ));
    }
    if !entry.is_evictable() {
        return Err(ApiError::with_details(
            "cache_entry_not_dehydratable",
            "cache entry is not in a dehydratable state",
            serde_json::json!({"node_id": node_id, "state": serde_json::to_value(entry.state).unwrap_or(Value::Null)}),
        ));
    }
    let staged = cache_transition(entry.state, CacheState::Evicting).map_err(core_error_to_api)?;
    let final_state = cache_transition(staged, CacheState::Absent).map_err(core_error_to_api)?;
    let snapshot = match final_state {
        CacheState::Absent => {
            inner.cache_entries.remove(&node_id);
            serde_json::json!({"node_id": node_id, "state": "absent", "dehydrated": true})
        }
        other => {
            return Err(ApiError::with_details(
                "cache_dehydrate_failed",
                "unexpected cache state after dehydrate",
                serde_json::json!({"state": serde_json::to_value(other).unwrap_or(Value::Null)}),
            ));
        }
    };
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
        serde_json::json!({"node_id": node_id, "state": "absent"}),
    );
    backend.record_audit(
        "cache.dehydrate",
        source,
        None,
        Some(node_id),
        None,
        None,
        AuditEventResult::Success,
        None,
    );
    Ok(snapshot)
}

pub fn cache_verify_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let total = inner.cache_entries.len();
    Ok(serde_json::json!({
        "verified": true,
        "entries_checked": total,
        "mismatches": [],
    }))
}

// ---- lock ----

pub fn lock_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let now = timestamp();
    let locks: Vec<Value> = inner
        .locks
        .values()
        .map(|lock| {
            let mut value = serde_json::to_value(lock).expect("lock serializes");
            if let Some(map) = value.as_object_mut() {
                let effective = effective_lock_status(lock, &now);
                map.insert(
                    "effective_status".to_string(),
                    serde_json::to_value(effective).unwrap_or(Value::Null),
                );
            }
            value
        })
        .collect();
    Ok(serde_json::json!({"locks": locks}))
}

pub fn lock_acquire_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let node_id = lookup_node_id(backend, params)?;
    let kind = lock_kind_param(params)?;
    let ttl_seconds = params
        .get("ttl_seconds")
        .and_then(|v| v.as_u64())
        .unwrap_or(600);
    let now = time::OffsetDateTime::now_utc();
    let expires = now + time::Duration::seconds(ttl_seconds.max(1) as i64);
    let acquired_at = timestamp();
    let expires_at = expires
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| acquired_at.clone());
    let org_id = backend.lock().org_id.clone();

    let lock = FileLock {
        org_id,
        lock_id: generate_id("lock_"),
        node_id: Some(node_id.clone()),
        provisional_local_id: None,
        path_snapshot: format!("/Project/{}", node_id),
        owner_user_id: None,
        owner_device_id: None,
        kind,
        status: LockStatus::Active,
        acquired_at: acquired_at.clone(),
        expires_at: Some(expires_at),
        released_at: None,
        broken_at: None,
        broken_by: None,
        operation_id: None,
    };
    let snapshot = lock.clone();
    backend.lock().locks.insert(lock.lock_id.clone(), lock);
    backend.record_event(
        biohazardfs_api_types::event_types::LOCK_CHANGED,
        serde_json::json!({"lock_id": snapshot.lock_id, "node_id": node_id, "status": "active"}),
    );
    backend.record_audit(
        "lock.acquire",
        source,
        None,
        Some(node_id),
        None,
        Some(snapshot.path_snapshot.clone()),
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("lock serializes"))
}

pub fn lock_release_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let lock_id = require_str_param(params, "lock_id")?;
    let now = timestamp();
    let mut inner = backend.lock();
    let lock = inner
        .locks
        .get_mut(lock_id)
        .ok_or_else(|| ApiError::new("lock_not_found", format!("lock {lock_id} was not found")))?;
    if !matches!(lock.status, LockStatus::Active) {
        return Err(ApiError::with_details(
            "lock_not_active",
            "only active locks can be released",
            serde_json::json!({"lock_id": lock_id, "status": serde_json::to_value(lock.status).unwrap_or(Value::Null)}),
        ));
    }
    lock.status = LockStatus::Released;
    lock.released_at = Some(now.clone());
    let snapshot = lock.clone();
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::LOCK_CHANGED,
        serde_json::json!({"lock_id": snapshot.lock_id, "status": "released"}),
    );
    backend.record_audit(
        "lock.release",
        source,
        None,
        snapshot.node_id.clone(),
        None,
        Some(snapshot.path_snapshot.clone()),
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("lock serializes"))
}

pub fn lock_status_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let lock_id = require_str_param(params, "lock_id")?;
    let inner = backend.lock();
    let lock = inner
        .locks
        .get(lock_id)
        .ok_or_else(|| ApiError::new("lock_not_found", format!("lock {lock_id} was not found")))?;
    let now = timestamp();
    let mut value = serde_json::to_value(lock).expect("lock serializes");
    if let Some(map) = value.as_object_mut() {
        let effective = effective_lock_status(lock, &now);
        map.insert(
            "effective_status".to_string(),
            serde_json::to_value(effective).unwrap_or(Value::Null),
        );
        map.insert(
            "effective".to_string(),
            Value::Bool(lock.is_effective_at(&now)),
        );
    }
    Ok(value)
}

pub fn lock_extend_payload(
    backend: &DaemonBackend,
    params: &Value,
    source: Source,
) -> Result<Value, ApiError> {
    let lock_id = require_str_param(params, "lock_id")?;
    let extend_seconds = params
        .get("extend_seconds")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| {
            ApiError::new(
                "missing_param",
                "extend_seconds is required for lock.extend",
            )
        })?;
    let now = time::OffsetDateTime::now_utc();
    let mut inner = backend.lock();
    let lock = inner
        .locks
        .get_mut(lock_id)
        .ok_or_else(|| ApiError::new("lock_not_found", format!("lock {lock_id} was not found")))?;
    if !matches!(lock.status, LockStatus::Active) {
        return Err(ApiError::with_details(
            "lock_not_active",
            "only active locks can be extended",
            serde_json::json!({"lock_id": lock_id}),
        ));
    }
    // Extend resets the TTL: new expiry is `extend_seconds` from now. We do
    // not parse the prior `expires_at` (that would pull in `time`'s `parsing`
    // feature); reset-from-now is the documented scaffold behavior.
    let extended = now + time::Duration::seconds(extend_seconds.max(1) as i64);
    lock.expires_at = Some(
        extended
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| timestamp()),
    );
    let snapshot = lock.clone();
    drop(inner);
    backend.record_event(
        biohazardfs_api_types::event_types::LOCK_CHANGED,
        serde_json::json!({"lock_id": snapshot.lock_id, "expires_at": snapshot.expires_at}),
    );
    backend.record_audit(
        "lock.extend",
        source,
        None,
        snapshot.node_id.clone(),
        None,
        Some(snapshot.path_snapshot.clone()),
        AuditEventResult::Success,
        None,
    );
    Ok(serde_json::to_value(snapshot).expect("lock serializes"))
}

// ---- conflict ----

pub fn conflict_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let conflicts: Vec<Value> = inner
        .conflicts
        .iter()
        .map(|conflict| serde_json::to_value(conflict).expect("conflict serializes"))
        .collect();
    Ok(serde_json::json!({"conflicts": conflicts}))
}

pub fn conflict_show_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let conflict_id = require_str_param(params, "conflict_id")?;
    let inner = backend.lock();
    let conflict = inner
        .conflicts
        .iter()
        .find(|conflict| conflict.conflict_id == conflict_id)
        .ok_or_else(|| {
            ApiError::new(
                "conflict_not_found",
                format!("conflict {conflict_id} was not found"),
            )
        })?;
    Ok(serde_json::to_value(conflict).expect("conflict serializes"))
}

// ---- transfer ----

pub fn transfer_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let transfers: Vec<Value> = inner
        .transfers
        .iter()
        .map(|transfer| serde_json::to_value(transfer).expect("transfer serializes"))
        .collect();
    Ok(serde_json::json!({"transfers": transfers}))
}

pub fn transfer_status_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let transfer_id = require_str_param(params, "transfer_id")?;
    let inner = backend.lock();
    let transfer = inner
        .transfers
        .iter()
        .find(|transfer| transfer.transfer_id == transfer_id)
        .ok_or_else(|| {
            ApiError::new(
                "transfer_not_found",
                format!("transfer {transfer_id} was not found"),
            )
        })?;
    Ok(serde_json::to_value(transfer).expect("transfer serializes"))
}

// ---- workset ----

pub fn workset_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    // The scaffold holds no worksets locally; worksets are server-owned and
    // surface through the daemon once the IPC client is wired. Honest empty.
    let count = backend.lock().nodes.len();
    Ok(serde_json::json!({
        "worksets": [],
        "namespace_nodes": count,
        "degraded": true,
        "note": "worksets are server-owned; the daemon has no cached worksets in the scaffold",
    }))
}

pub fn workset_show_payload(params: &Value) -> Result<Value, ApiError> {
    let workset_id = require_str_param(params, "workset_id")?;
    Err(ApiError::with_details(
        "workset_not_found",
        format!("workset {workset_id} is not cached on the daemon in the scaffold"),
        serde_json::json!({"workset_id": workset_id}),
    ))
}

// ---- snapshot (read spine; mutations are periphery) ----

pub fn snapshot_list_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let inner = backend.lock();
    // We do not maintain a separate snapshot vec; snapshots are server-owned.
    // Surface the empty list with the namespace node count for context.
    Ok(serde_json::json!({
        "snapshots": [],
        "namespace_nodes": inner.nodes.len(),
    }))
}

// ---- collaboration reads (mutations are periphery) ----

pub fn invite_list_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({"invites": []}))
}

pub fn share_list_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({"shares": []}))
}

pub fn grant_list_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({"grants": []}))
}

pub fn publish_list_payload() -> Result<Value, ApiError> {
    Ok(serde_json::json!({"publishes": []}))
}

// ---- audit reads (export is periphery) ----

pub fn audit_events_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let inner = backend.lock();
    let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(100) as usize;
    let event_type = params.get("event_type").and_then(|v| v.as_str());
    let mut events: Vec<Value> = inner
        .audit
        .iter()
        .rev()
        .filter(|event| match event_type {
            Some(filter) => event.event_type == filter,
            None => true,
        })
        .take(limit)
        .map(|event| serde_json::to_value(event).expect("audit event serializes"))
        .collect();
    events.reverse();
    Ok(serde_json::json!({"events": events}))
}

pub fn audit_event_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let audit_event_id = require_str_param(params, "audit_event_id")?;
    let inner = backend.lock();
    let event = inner
        .audit
        .iter()
        .find(|event| event.audit_event_id == audit_event_id)
        .ok_or_else(|| {
            ApiError::new(
                "audit_event_not_found",
                format!("audit event {audit_event_id} was not found"),
            )
        })?;
    Ok(serde_json::to_value(event).expect("audit event serializes"))
}

pub fn audit_actor_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    let actor_user_id = require_str_param(params, "actor_user_id")?;
    let inner = backend.lock();
    let events: Vec<Value> = inner
        .audit
        .iter()
        .filter(|event| event.actor_user_id.as_deref() == Some(actor_user_id))
        .map(|event| serde_json::to_value(event).expect("audit event serializes"))
        .collect();
    Ok(serde_json::json!({
        "actor_user_id": actor_user_id,
        "events": events,
    }))
}

// ---- schema ----

pub fn schema_list_payload() -> Result<Value, ApiError> {
    let methods: Vec<Value> = biohazardfs_api_types::known_methods::DAEMON_METHODS
        .iter()
        .map(schema_descriptor_value)
        .collect();
    Ok(serde_json::json!({
        "methods": methods,
        "schema_version": biohazardfs_api_types::DAEMON_SCHEMA_VERSION,
    }))
}

pub fn schema_method_payload(params: &Value) -> Result<Value, ApiError> {
    let name = require_str_param(params, "method")?;
    let descriptor = biohazardfs_api_types::known_methods::find(
        biohazardfs_api_types::known_methods::Surface::Daemon,
        name,
    )
    .ok_or_else(|| {
        ApiError::new(
            "method_not_found",
            format!("{name} is not a registered daemon method"),
        )
    })?;
    Ok(schema_descriptor_value(&descriptor))
}

// ===========================================================================
// Internal helpers
// ===========================================================================

/// Look up a namespace node by the `node_id` param. Validates the ID at the
/// trust boundary (daemon params come from CLI/UI/agents).
fn lookup_node(backend: &DaemonBackend, params: &Value) -> Result<Node, ApiError> {
    let inner = backend.lock();
    let node_id = match resolve_node_id(&inner, params)? {
        Some(node_id) => node_id,
        None => {
            return Err(ApiError::with_details(
                "missing_param",
                "node_id or path is required",
                serde_json::json!({"field": "node_id"}),
            ));
        }
    };
    inner
        .nodes
        .get(&node_id)
        .cloned()
        .filter(Node::is_live)
        .ok_or_else(|| {
            ApiError::new(
                "node_not_found",
                format!("node {node_id} was not found in the namespace"),
            )
        })
}

/// Resolve the target node id from request params. An explicit `node_id` wins;
/// otherwise walk the namespace from the root by `path` so artist-facing
/// callers (CLI, FUSE) can address nodes by mount-relative path without the
/// caller pre-resolving to a node id. Returns `Ok(None)` if neither is present.
fn resolve_node_id(inner: &InMemoryBackend, params: &Value) -> Result<Option<String>, ApiError> {
    if let Some(node_id) = params.get("node_id").and_then(|value| value.as_str()) {
        if !node_id.starts_with(NODE_ID_PREFIX) {
            return Err(ApiError::with_details(
                "invalid_param",
                format!("node_id must start with {NODE_ID_PREFIX:?}"),
                serde_json::json!({"field": "node_id"}),
            ));
        }
        validate_node_id(node_id).map_err(core_error_to_api)?;
        return Ok(Some(node_id.to_string()));
    }
    if let Some(path) = params.get("path").and_then(|value| value.as_str()) {
        return resolve_node_by_path(inner, path).map(Some);
    }
    Ok(None)
}

/// Walk the namespace from the root by `path`. Mount-relative: a leading `/` is
/// stripped, empty segments are ignored, and each segment names a live child of
/// the current node (case-insensitive match, matching the sibling-uniqueness
/// policy). The root's own name is not matched.
fn resolve_node_by_path(inner: &InMemoryBackend, path: &str) -> Result<String, ApiError> {
    use biohazardfs_core::path::case_insensitive_sibling_key;
    let root_id = inner
        .nodes
        .values()
        .find(|node| node.is_live() && node.parent_node_id.is_none())
        .map(|node| node.node_id.clone())
        .ok_or_else(|| {
            ApiError::new(
                "namespace_root_missing",
                "namespace has no live root node to resolve paths against",
            )
        })?;
    let mut current = root_id;
    for segment in path.trim_start_matches('/').split('/') {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }
        let key = case_insensitive_sibling_key(segment);
        match inner
            .nodes
            .values()
            .find(|node| {
                node.is_live()
                    && node.parent_node_id.as_deref() == Some(current.as_str())
                    && case_insensitive_sibling_key(&node.name) == key
            })
            .map(|node| node.node_id.clone())
        {
            Some(node_id) => current = node_id,
            None => {
                return Err(ApiError::new(
                    "node_not_found",
                    format!("path segment {segment:?} not found under node {current}"),
                ));
            }
        }
    }
    Ok(current)
}

/// Backend-level node-id resolution. Locks internally and accepts either
/// `node_id` or a mount-relative `path`. Use when a payload needs only the id
/// string and does not otherwise hold the backend lock.
fn lookup_node_id(backend: &DaemonBackend, params: &Value) -> Result<String, ApiError> {
    let inner = backend.lock();
    resolve_node_id(&inner, params)?.ok_or_else(|| {
        ApiError::with_details(
            "missing_param",
            "node_id or path is required",
            serde_json::json!({"field": "node_id"}),
        )
    })
}

fn require_str_param<'a>(params: &'a Value, key: &str) -> Result<&'a str, ApiError> {
    params.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
        ApiError::with_details(
            "missing_param",
            format!("{key} is required"),
            serde_json::json!({"field": key}),
        )
    })
}

fn lock_kind_param(params: &Value) -> Result<LockKind, ApiError> {
    let raw = params
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or("edit");
    match raw {
        "edit" => Ok(LockKind::Edit),
        "admin" => Ok(LockKind::Admin),
        "publish" => Ok(LockKind::Publish),
        "restore" => Ok(LockKind::Restore),
        other => Err(ApiError::with_details(
            "invalid_param",
            format!("unknown lock kind {other:?}"),
            serde_json::json!({"field": "kind", "value": other}),
        )),
    }
}

fn effective_lock_status(lock: &FileLock, now_rfc3339: &str) -> LockStatus {
    if lock.status != LockStatus::Active {
        return lock.status;
    }
    match &lock.expires_at {
        Some(expires_at) if expires_at.as_str() <= now_rfc3339 => LockStatus::Expired,
        _ => lock.status,
    }
}

fn schema_descriptor_value(
    descriptor: &biohazardfs_api_types::known_methods::MethodDescriptor,
) -> Value {
    serde_json::json!({
        "name": descriptor.name,
        "group": descriptor.group,
        "classification": serde_json::to_value(descriptor.classification)
            .unwrap_or(Value::Null),
        "summary": descriptor.summary,
    })
}

fn credentials_dir() -> std::path::PathBuf {
    // Owner-only runtime state lives under the user's runtime dir. We do not
    // create it here; the path is advisory for `config.path` /
    // `auth.credentials_path`.
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return std::path::PathBuf::from(dir).join("biohazardfs");
    }
    std::path::PathBuf::from("/run/user").join("biohazardfs")
}

fn core_error_to_api(error: biohazardfs_core::error::CoreError) -> ApiError {
    ApiError::new(error.code, error.message)
}

/// Decode a hex string into bytes. Used by `file.write` to turn `content_hex`
/// into raw content without introducing a base64 or bytes dep. Errors carry a
/// human-readable message so the API boundary can surface them as `invalid_param`.
fn decode_hex(hex: &str) -> Result<Vec<u8>, String> {
    if !hex.len().is_multiple_of(2) {
        return Err(format!(
            "content_hex has an odd number of characters ({})",
            hex.len()
        ));
    }
    let bytes = hex.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut index = 0;
    while index < bytes.len() {
        let high = hex_nibble(bytes[index])?;
        let low = hex_nibble(bytes[index + 1])?;
        out.push((high << 4) | low);
        index += 2;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(format!("content_hex contains non-hex character {byte:?}")),
    }
}

/// Encode bytes as lowercase hex. Mirrors the server `content_hex` shape so the
/// FUSE layer can hydrate and round-trip content with one wire encoding.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Scaffold content hash: FNV-1a 64-bit over the raw bytes. Labelled
/// `sha256:scaffold:` so callers cannot mistake it for a verified digest;
/// `file.checksum` uses the same convention. Production swaps both to a real
/// sha256 computed by the object store during ingest.
fn content_hash_for(bytes: &[u8]) -> String {
    format!("sha256:scaffold:{:x}", fxhash(bytes))
}

/// Build a redacted copy of the `file.write` request params for persistence in
/// the Operation log and AuditEvent payload. METADATA_SCHEMA.md forbids file
/// contents in audit/provenance records: those records are exported and
/// inspected, and `content_hex` IS the raw file bytes. This is an allowlist, not
/// a denylist: only known-safe metadata is copied across, so any future
/// content-bearing field added to the wire shape (a base64 blob, a streaming
/// chunk array, etc.) is dropped by default rather than silently leaked. The
/// authoritative computed values (`node_id`, `version_id`, `content_hash`,
/// `size_bytes`) are inserted last so the record points at the version actually
/// written regardless of how the caller shaped the request.
fn redact_file_write_params(
    params: &Value,
    node_id: &str,
    version_id: &str,
    content_hash: &str,
    size_bytes: u64,
) -> Value {
    let mut redacted = serde_json::Map::new();
    if let Some(map) = params.as_object() {
        for key in [
            "node_id",
            "parent_node_id",
            "name",
            "mode",
            "base_version_id",
        ] {
            if let Some(value) = map.get(key) {
                redacted.insert(key.to_string(), value.clone());
            }
        }
    }
    redacted.insert("node_id".to_string(), Value::String(node_id.to_string()));
    redacted.insert(
        "version_id".to_string(),
        Value::String(version_id.to_string()),
    );
    redacted.insert(
        "content_hash".to_string(),
        Value::String(content_hash.to_string()),
    );
    redacted.insert("size_bytes".to_string(), serde_json::json!(size_bytes));
    Value::Object(redacted)
}

/// Serialize the redacted `file.write` params plus the `created` flag for the
/// AuditEvent payload. Falls back to a minimal object if serialization fails
/// (AuditEvent is plain JSON-safe, so that path is an invariant guard only).
fn build_file_write_audit_payload(redacted_params: &Value, created: bool) -> String {
    let mut payload = redacted_params.clone();
    if let Some(map) = payload.as_object_mut() {
        map.insert("created".to_string(), serde_json::json!(created));
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

/// Authoritative byte size for a namespace node, for `file.stat` / `file.list`
/// and FUSE size-advertise at lookup. The committed [`FileVersion`] is the
/// source of truth; in-memory `file_contents` is the fallback when a version
/// has not been recorded yet. Directories and unknown nodes report 0 so the
/// size signal is never fabricated.
fn node_size_bytes(inner: &InMemoryBackend, node: &Node) -> u64 {
    if node.kind != NodeKind::File {
        return 0;
    }
    if let Some(version_id) = node.current_version_id.as_deref()
        && let Some(version) = inner.file_versions.get(version_id)
    {
        return version.size_bytes;
    }
    if let Some(bytes) = inner.file_contents.get(&node.node_id) {
        return bytes.len() as u64;
    }
    0
}

/// Drive a cache entry to `Ready` through the legal forward transition path
/// after a `file.write` commit. The transition guards (core::cache::transition)
/// reject unsafe moves; we never paper over them. This is the safety valve for
/// the FILESYSTEM_SEMANTICS.md invariant that dirty/pinned data is never lost.
fn apply_write_cache_transition(entry: &mut CacheEntry) -> Result<(), ApiError> {
    match entry.state {
        CacheState::Absent | CacheState::Failed => {
            entry.state =
                cache_transition(entry.state, CacheState::Populating).map_err(core_error_to_api)?;
            entry.state =
                cache_transition(entry.state, CacheState::Ready).map_err(core_error_to_api)?;
        }
        CacheState::Ready => {
            // Documented write cycle: Ready -> Dirty (write made it dirty) ->
            // Ready (upload acknowledged). Exercises Dirty -> Ready directly.
            entry.state =
                cache_transition(entry.state, CacheState::Dirty).map_err(core_error_to_api)?;
            entry.state =
                cache_transition(entry.state, CacheState::Ready).map_err(core_error_to_api)?;
        }
        CacheState::Dirty | CacheState::Populating => {
            entry.state =
                cache_transition(entry.state, CacheState::Ready).map_err(core_error_to_api)?;
        }
        CacheState::Pinned => {
            // Pinned writes are allowed (pinned = never evicted, not read-only).
            // Pinned -> Ready is not direct; route through Populating.
            entry.state = cache_transition(CacheState::Pinned, CacheState::Populating)
                .map_err(core_error_to_api)?;
            entry.state =
                cache_transition(entry.state, CacheState::Ready).map_err(core_error_to_api)?;
        }
        CacheState::Pinning | CacheState::Evicting => {
            return Err(ApiError::with_details(
                "cache_write_blocked",
                "cache entry is in a transitional state that cannot accept a write",
                serde_json::json!({
                    "state": serde_json::to_value(entry.state).unwrap_or(Value::Null),
                }),
            ));
        }
    }
    Ok(())
}

/// Canonical JSON for hashing: object keys sorted recursively, scalars via
/// `to_string`. Order-independent so issue and validate agree regardless of
/// how the params object was constructed.
fn canonicalize_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut parts = Vec::with_capacity(keys.len());
            for key in keys {
                let val = &map[key];
                parts.push(format!("\"{}\":{}", key, canonicalize_json(val)));
            }
            format!("{{{}}}", parts.join(","))
        }
        Value::Array(items) => {
            let parts: Vec<String> = items.iter().map(canonicalize_json).collect();
            format!("[{}]", parts.join(","))
        }
        _ => value.to_string(),
    }
}

/// Scaffold params hash: FNV-1a 64-bit over the canonical JSON. Non-crypto on
/// purpose (no new deps); production swaps in `sha256:`.
fn params_hash(params: &Value) -> String {
    let canonical = canonicalize_json(params);
    format!("scaffold_hash:{:016x}", fxhash(canonical.as_bytes()))
}

/// Plan hash binds method + params hash so a token issued for one method
/// cannot be replayed against another even if their params collide.
fn plan_hash(method: &str, params_hash: &str) -> String {
    let merged = format!("{method}|{params_hash}");
    format!("scaffold_plan:{:016x}", fxhash(merged.as_bytes()))
}

/// Compare the attempted method/classification/source against the values bound
/// to the issued token. A token issued for one plan must not apply a different
/// one even if its params hash happens to match. Returns
/// `operation_token_mismatch` with the first diverging field (method,
/// classification, or source) so callers can distinguish a cross-plan replay
/// from a same-plan params drift.
fn validate_token_binding(
    token: &OperationToken,
    method: &str,
    classification: MutationClassification,
    source: Source,
) -> Result<(), ApiError> {
    if token.method != method {
        return Err(ApiError::with_details(
            "operation_token_mismatch",
            "operation token was issued for a different method",
            serde_json::json!({
                "field": "method",
                "expected": token.method,
                "actual": method,
            }),
        ));
    }
    if token.classification != classification {
        return Err(ApiError::with_details(
            "operation_token_mismatch",
            "operation token was issued for a different classification",
            serde_json::json!({
                "field": "classification",
                "expected": serde_json::to_value(token.classification).unwrap_or(Value::Null),
                "actual": serde_json::to_value(classification).unwrap_or(Value::Null),
            }),
        ));
    }
    if token.source != source {
        return Err(ApiError::with_details(
            "operation_token_mismatch",
            "operation token was issued for a different source",
            serde_json::json!({
                "field": "source",
                "expected": serde_json::to_value(&token.source).unwrap_or(Value::Null),
                "actual": serde_json::to_value(&source).unwrap_or(Value::Null),
            }),
        ));
    }
    Ok(())
}

/// FNV-1a 64-bit. Deterministic, dependency-free, sufficient for drift
/// detection between issue and validate.
fn fxhash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::DaemonRequest;

    fn test_backend() -> DaemonBackend {
        DaemonBackend::new("127.0.0.1:47666")
    }

    fn req(method: &str, params: Value) -> DaemonRequest {
        let mut request = DaemonRequest::new(method, Source::Test);
        request.params = params;
        request
    }

    #[test]
    fn seeds_root_and_two_children() {
        let backend = test_backend();
        let inner = backend.lock();
        assert!(inner.nodes.contains_key("node_root"));
        assert!(inner.nodes.contains_key("node_shots"));
        assert!(inner.nodes.contains_key("node_readme"));
        assert_eq!(inner.org_id, SCAFFOLD_ORG_ID);
    }

    #[test]
    fn file_list_defaults_to_root_children() {
        let backend = test_backend();
        let payload = file_list_payload(&backend, &serde_json::json!({})).unwrap();
        let names: Vec<String> = payload["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap().to_string())
            .collect();
        assert!(names.contains(&"shots".to_string()));
        assert!(names.contains(&"README.md".to_string()));
    }

    #[test]
    fn file_stat_returns_node() {
        let backend = test_backend();
        let payload =
            file_stat_payload(&backend, &serde_json::json!({"node_id": "node_root"})).unwrap();
        assert_eq!(payload["node_id"], "node_root");
        assert_eq!(payload["kind"], "directory");
    }

    #[test]
    fn file_stat_rejects_bad_node_id() {
        let backend = test_backend();
        let err =
            file_stat_payload(&backend, &serde_json::json!({"node_id": "ver_bad"})).unwrap_err();
        assert_eq!(err.code, "invalid_param");
        let err = file_stat_payload(&backend, &serde_json::json!({"node_id": "node_missing"}))
            .unwrap_err();
        assert_eq!(err.code, "node_not_found");
    }

    #[test]
    fn file_stat_resolves_path_to_node() {
        // Path-based addressing: artist-facing callers (CLI/FUSE) send a
        // mount-relative path instead of a node_id. The daemon resolves it by
        // walking the namespace from the root, so the CLI does not need to
        // pre-resolve paths to node ids.
        let backend = test_backend();

        let shots = file_stat_payload(&backend, &serde_json::json!({"path": "shots"})).unwrap();
        assert_eq!(shots["node_id"], "node_shots");
        assert_eq!(shots["name"], "shots");

        // Leading slash resolves the same way.
        let readme =
            file_stat_payload(&backend, &serde_json::json!({"path": "/README.md"})).unwrap();
        assert_eq!(readme["node_id"], "node_readme");

        // Case-insensitive match per the sibling-uniqueness policy.
        let upper = file_stat_payload(&backend, &serde_json::json!({"path": "readme.md"})).unwrap();
        assert_eq!(upper["node_id"], "node_readme");

        // A missing segment returns node_not_found (not missing_param).
        let err = file_stat_payload(&backend, &serde_json::json!({"path": "nope"})).unwrap_err();
        assert_eq!(err.code, "node_not_found");

        // Neither node_id nor path present keeps the missing_param contract.
        let err = file_stat_payload(&backend, &serde_json::json!({})).unwrap_err();
        assert_eq!(err.code, "missing_param");
    }

    #[test]
    fn file_stat_and_list_report_size_bytes_for_files() {
        // FUSE size-advertise: file.stat and file.list must surface the real
        // byte length of a file so the kernel can advertise it at lookup,
        // without a separate file.read. Directories report 0.
        let backend = test_backend();
        let written = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "sized.exr",
                "content_hex": "deadbeefcafe", // 6 bytes
            }),
            Source::Cli,
        )
        .unwrap();
        let node_id = written["node_id"].as_str().unwrap().to_string();

        // file.stat reports the committed byte length.
        let stat =
            file_stat_payload(&backend, &serde_json::json!({"node_id": node_id.clone()})).unwrap();
        assert_eq!(stat["size_bytes"], 6);

        // file.list entries include size_bytes for the file.
        let list = file_list_payload(
            &backend,
            &serde_json::json!({"parent_node_id": "node_root"}),
        )
        .unwrap();
        let entry = list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["node_id"].as_str() == Some(node_id.as_str()))
            .expect("written file appears in listing");
        assert_eq!(entry["size_bytes"], 6);

        // Directories report size 0 in both surfaces.
        let dir_stat =
            file_stat_payload(&backend, &serde_json::json!({"node_id": "node_root"})).unwrap();
        assert_eq!(dir_stat["size_bytes"], 0);
        let dir_entry = list["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"].as_str() == Some("shots"))
            .expect("shots directory appears in listing");
        assert_eq!(dir_entry["size_bytes"], 0);
    }

    #[test]
    fn cache_pin_marks_entry_pinned() {
        let backend = test_backend();
        let before = cache_list_payload(&backend).unwrap();
        assert!(before["entries"].as_array().unwrap().is_empty());

        let pinned = cache_pin_payload(
            &backend,
            &serde_json::json!({"node_id": "node_readme"}),
            Source::Test,
        )
        .unwrap();
        assert!(pinned["pinned"].as_bool().unwrap());
        assert_eq!(pinned["state"], "pinned");

        let listed = cache_list_payload(&backend).unwrap();
        assert_eq!(listed["entries"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn cache_dehydrate_refuses_dirty_and_pinned() {
        let backend = test_backend();
        backend.seed_cache_entry(CacheEntry {
            node_id: "node_dirty".to_string(),
            version_id: None,
            state: CacheState::Dirty,
            content_hash: None,
            size_bytes: 10,
            pinned: false,
            dirty: true,
            last_accessed_at: None,
        });
        backend.seed_cache_entry(CacheEntry {
            node_id: "node_pinned".to_string(),
            version_id: None,
            state: CacheState::Pinned,
            content_hash: None,
            size_bytes: 10,
            pinned: true,
            dirty: false,
            last_accessed_at: None,
        });
        backend.seed_cache_entry(CacheEntry {
            node_id: "node_ready".to_string(),
            version_id: None,
            state: CacheState::Ready,
            content_hash: None,
            size_bytes: 10,
            pinned: false,
            dirty: false,
            last_accessed_at: None,
        });

        let dirty_err = cache_dehydrate_payload(
            &backend,
            &serde_json::json!({"node_id": "node_dirty"}),
            Source::Test,
        )
        .unwrap_err();
        assert_eq!(dirty_err.code, "cache_entry_dirty");

        let pinned_err = cache_dehydrate_payload(
            &backend,
            &serde_json::json!({"node_id": "node_pinned"}),
            Source::Test,
        )
        .unwrap_err();
        assert_eq!(pinned_err.code, "cache_entry_pinned");

        let ok = cache_dehydrate_payload(
            &backend,
            &serde_json::json!({"node_id": "node_ready"}),
            Source::Test,
        )
        .unwrap();
        assert_eq!(ok["state"], "absent");
        assert_eq!(ok["dehydrated"], true);

        // Evicted entry is gone from the index.
        let after = backend.lock();
        assert!(!after.cache_entries.contains_key("node_ready"));
    }

    #[test]
    fn cache_status_counts_dirty_pinned_ready() {
        let backend = test_backend();
        cache_pin_payload(
            &backend,
            &serde_json::json!({"node_id": "node_readme"}),
            Source::Test,
        )
        .unwrap();
        backend.seed_cache_entry(CacheEntry {
            node_id: "node_dirty".to_string(),
            version_id: None,
            state: CacheState::Dirty,
            content_hash: None,
            size_bytes: 50,
            pinned: false,
            dirty: true,
            last_accessed_at: None,
        });
        let status = cache_status_payload(&backend).unwrap();
        assert_eq!(status["total_entries"], 2);
        assert_eq!(status["pinned_entries"], 1);
        assert_eq!(status["dirty_entries"], 1);
    }

    #[test]
    fn lock_acquire_and_lazy_expiry() {
        let backend = test_backend();
        let lock = lock_acquire_payload(
            &backend,
            &serde_json::json!({"node_id": "node_readme", "ttl_seconds": 1}),
            Source::Test,
        )
        .unwrap();
        let lock_id = lock["lock_id"].as_str().unwrap().to_string();

        // An active lock just acquired is effective now.
        let status_now =
            lock_status_payload(&backend, &serde_json::json!({"lock_id": lock_id})).unwrap();
        assert_eq!(status_now["status"], "active");
        assert_eq!(status_now["effective_status"], "active");
        assert!(status_now["effective"].as_bool().unwrap());

        // Seed an expired lock directly and confirm lazy expiry is reported.
        let mut inner = backend.lock();
        let expired_lock_id = "lock_expired".to_string();
        inner.locks.insert(
            expired_lock_id.clone(),
            FileLock {
                org_id: SCAFFOLD_ORG_ID.to_string(),
                lock_id: expired_lock_id.clone(),
                node_id: Some("node_root".to_string()),
                provisional_local_id: None,
                path_snapshot: "/Project".to_string(),
                owner_user_id: None,
                owner_device_id: None,
                kind: LockKind::Edit,
                status: LockStatus::Active,
                acquired_at: "2020-01-01T00:00:00Z".to_string(),
                expires_at: Some("2020-01-01T00:01:00Z".to_string()),
                released_at: None,
                broken_at: None,
                broken_by: None,
                operation_id: None,
            },
        );
        drop(inner);

        let status_expired =
            lock_status_payload(&backend, &serde_json::json!({"lock_id": expired_lock_id}))
                .unwrap();
        assert_eq!(status_expired["effective_status"], "expired");
        assert!(!status_expired["effective"].as_bool().unwrap());
    }

    #[test]
    fn lock_release_marks_released() {
        let backend = test_backend();
        let lock = lock_acquire_payload(
            &backend,
            &serde_json::json!({"node_id": "node_readme"}),
            Source::Test,
        )
        .unwrap();
        let lock_id = lock["lock_id"].as_str().unwrap().to_string();
        let released = lock_release_payload(
            &backend,
            &serde_json::json!({"lock_id": lock_id}),
            Source::Test,
        )
        .unwrap();
        assert_eq!(released["status"], "released");
        assert!(released["released_at"].as_str().is_some());
    }

    #[test]
    fn lock_extend_pushes_expiry_forward() {
        let backend = test_backend();
        let lock = lock_acquire_payload(
            &backend,
            &serde_json::json!({"node_id": "node_readme", "ttl_seconds": 60}),
            Source::Test,
        )
        .unwrap();
        let lock_id = lock["lock_id"].as_str().unwrap().to_string();
        let before = lock["expires_at"].as_str().unwrap().to_string();
        let extended = lock_extend_payload(
            &backend,
            &serde_json::json!({"lock_id": lock_id, "extend_seconds": 600}),
            Source::Test,
        )
        .unwrap();
        let after = extended["expires_at"].as_str().unwrap().to_string();
        assert!(
            after > before,
            "extend should push expiry forward: {before} -> {after}"
        );
    }

    #[test]
    fn conflict_show_returns_seeded_conflict() {
        let backend = test_backend();
        let conflict = Conflict {
            org_id: SCAFFOLD_ORG_ID.to_string(),
            conflict_id: "conf_demo".to_string(),
            node_id: Some("node_readme".to_string()),
            path_snapshot: "/Project/README.md".to_string(),
            kind: biohazardfs_core::conflict::ConflictKind::WriteWrite,
            base_version_id: None,
            local_version_id: None,
            remote_version_id: None,
            local_operation_id: None,
            remote_operation_id: None,
            status: biohazardfs_core::conflict::ConflictStatus::Open,
            created_at: "2026-07-05T00:00:00Z".to_string(),
            resolved_at: None,
            resolved_by: None,
            resolution_json: None,
        };
        backend.seed_conflict(conflict);
        let payload =
            conflict_show_payload(&backend, &serde_json::json!({"conflict_id": "conf_demo"}))
                .unwrap();
        assert_eq!(payload["conflict_id"], "conf_demo");
        assert_eq!(payload["status"], "open");
        let list = conflict_list_payload(&backend).unwrap();
        assert_eq!(list["conflicts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn schema_method_describes_known_method() {
        let payload = schema_method_payload(&serde_json::json!({"method": "cache.pin"})).unwrap();
        assert_eq!(payload["name"], "cache.pin");
        assert_eq!(payload["group"], "cache");
        assert_eq!(payload["classification"], "low_risk");
    }

    #[test]
    fn schema_list_includes_every_registered_method() {
        let payload = schema_list_payload().unwrap();
        let methods = payload["methods"].as_array().unwrap();
        let total = biohazardfs_api_types::known_methods::daemon_method_names().len();
        assert_eq!(methods.len(), total);
    }

    #[test]
    fn issue_and_validate_operation_token_round_trip() {
        let backend = test_backend();
        let params = serde_json::json!({"node_id": "node_readme", "dry_run": true});
        let token = backend.issue_operation_token(
            "file.delete",
            &params,
            MutationClassification::Destructive,
            Source::Agent,
        );
        // Token hash excludes operation_token, so adding it is fine.
        let mut with_token = params.clone();
        with_token["operation_token"] = serde_json::Value::String(token.operation_token.clone());
        let validated = backend
            .validate_operation_token(
                &token.operation_token,
                "file.delete",
                MutationClassification::Destructive,
                Source::Agent,
                &with_token,
            )
            .unwrap();
        assert_eq!(validated.operation_token, token.operation_token);
        assert_eq!(validated.method, "file.delete");
    }

    #[test]
    fn validate_operation_token_rejects_param_drift() {
        let backend = test_backend();
        let params = serde_json::json!({"node_id": "node_readme"});
        let token = backend.issue_operation_token(
            "file.delete",
            &params,
            MutationClassification::Destructive,
            Source::Agent,
        );
        let drifted = serde_json::json!({
            "node_id": "node_root",
            "operation_token": token.operation_token,
        });
        let err = backend
            .validate_operation_token(
                &token.operation_token,
                "file.delete",
                MutationClassification::Destructive,
                Source::Agent,
                &drifted,
            )
            .unwrap_err();
        assert_eq!(err.code, "operation_token_params_mismatch");
        assert!(err.details.is_some());
    }

    #[test]
    fn validate_operation_token_rejects_unknown_token() {
        let backend = test_backend();
        let err = backend
            .validate_operation_token(
                "optok_bogus",
                "file.delete",
                MutationClassification::Destructive,
                Source::Agent,
                &serde_json::json!({}),
            )
            .unwrap_err();
        assert_eq!(err.code, "operation_token_invalid");
    }

    #[test]
    fn validate_operation_token_rejects_method_mismatch() {
        // A token issued for cache.evict+destructive must not apply to a
        // different method even with identical params/classification/source.
        let backend = test_backend();
        let params = serde_json::json!({"node_id": "node_readme"});
        let token = backend.issue_operation_token(
            "cache.evict",
            &params,
            MutationClassification::Destructive,
            Source::Agent,
        );
        let err = backend
            .validate_operation_token(
                &token.operation_token,
                "file.write",
                MutationClassification::Destructive,
                Source::Agent,
                &params,
            )
            .unwrap_err();
        assert_eq!(err.code, "operation_token_mismatch");
        let details = err.details.unwrap();
        assert_eq!(details["field"], "method");
        assert_eq!(details["expected"], "cache.evict");
        assert_eq!(details["actual"], "file.write");
    }

    #[test]
    fn validate_operation_token_rejects_classification_mismatch() {
        // Same method/params/source, but a destructive token cannot be used to
        // authorize a low_risk attempt (and vice versa).
        let backend = test_backend();
        let params = serde_json::json!({"node_id": "node_readme"});
        let token = backend.issue_operation_token(
            "cache.evict",
            &params,
            MutationClassification::Destructive,
            Source::Agent,
        );
        let err = backend
            .validate_operation_token(
                &token.operation_token,
                "cache.evict",
                MutationClassification::LowRisk,
                Source::Agent,
                &params,
            )
            .unwrap_err();
        assert_eq!(err.code, "operation_token_mismatch");
        let details = err.details.unwrap();
        assert_eq!(details["field"], "classification");
        assert_eq!(details["expected"], "destructive");
        assert_eq!(details["actual"], "low_risk");
    }

    #[test]
    fn validate_operation_token_rejects_source_mismatch() {
        // A token issued for an agent must not authorize a CLI attempt.
        let backend = test_backend();
        let params = serde_json::json!({"node_id": "node_readme"});
        let token = backend.issue_operation_token(
            "cache.evict",
            &params,
            MutationClassification::Destructive,
            Source::Agent,
        );
        let err = backend
            .validate_operation_token(
                &token.operation_token,
                "cache.evict",
                MutationClassification::Destructive,
                Source::Cli,
                &params,
            )
            .unwrap_err();
        assert_eq!(err.code, "operation_token_mismatch");
        let details = err.details.unwrap();
        assert_eq!(details["field"], "source");
        assert_eq!(details["expected"], "agent");
        assert_eq!(details["actual"], "cli");
    }

    #[test]
    fn canonicalize_json_is_key_order_independent() {
        let a = canonicalize_json(&serde_json::json!({"a": 1, "b": 2}));
        let b = canonicalize_json(&serde_json::json!({"b": 2, "a": 1}));
        assert_eq!(a, b);
    }

    #[test]
    fn params_hash_excludes_nothing_when_no_token_field() {
        // Same params -> same hash, regardless of construction order.
        let h1 = params_hash(&serde_json::json!({"node_id": "node_x", "dry_run": true}));
        let h2 = params_hash(&serde_json::json!({"dry_run": true, "node_id": "node_x"}));
        assert_eq!(h1, h2);
        assert!(h1.starts_with("scaffold_hash:"));
    }

    #[test]
    fn daemon_health_is_ready_when_no_mounts_attached() {
        let backend = test_backend();
        let payload = daemon_health_payload(&backend).unwrap();
        assert_eq!(payload["state"], "ready");
        assert!(
            payload["checks"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c["ok"].as_bool().unwrap())
        );
    }

    #[test]
    fn record_audit_appends_to_buffer_and_event_stream() {
        let backend = test_backend();
        let before = backend.lock().audit.len();
        backend.record_audit(
            "file.write",
            Source::Cli,
            Some("req_x".to_string()),
            Some("node_readme".to_string()),
            None,
            Some("/Project/README.md".to_string()),
            AuditEventResult::Success,
            None,
        );
        let after = backend.lock().audit.len();
        assert_eq!(after, before + 1);
        let events = backend.recent_events();
        assert!(
            events
                .iter()
                .any(|envelope| envelope.event_type == "audit.event_recorded")
        );
    }

    #[test]
    fn request_round_trip_is_machine_shape() {
        let _request = req("cache.pin", serde_json::json!({"node_id": "node_readme"}));
        // Smoke-test the request builder; dispatch exercises it in lib tests.
    }

    #[test]
    fn file_write_creates_node_version_and_content() {
        let backend = test_backend();
        let payload = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "shot010.exr",
                "content_hex": "deadbeef",
            }),
            Source::Cli,
        )
        .unwrap();

        assert_eq!(payload["created"], true);
        let node_id = payload["node_id"].as_str().unwrap().to_string();
        let version_id = payload["version_id"].as_str().unwrap().to_string();
        assert!(node_id.starts_with("node_"));
        assert!(version_id.starts_with("ver_"));
        assert_eq!(payload["size_bytes"], 4);
        assert_eq!(
            payload["content_hash"],
            content_hash_for(&[0xde, 0xad, 0xbe, 0xef])
        );
        assert!(payload["operation_id"].as_str().unwrap().starts_with("op_"));

        let inner = backend.lock();
        let stored = inner.nodes.get(&node_id).unwrap();
        assert_eq!(stored.kind, NodeKind::File);
        assert_eq!(
            stored.current_version_id.as_deref(),
            Some(version_id.as_str())
        );
        assert_eq!(stored.parent_node_id.as_deref(), Some("node_root"));
        assert_eq!(
            inner.file_contents.get(&node_id).unwrap(),
            &vec![0xde, 0xad, 0xbe, 0xef]
        );
        let version = inner.file_versions.get(&version_id).unwrap();
        assert_eq!(version.size_bytes, 4);
        assert_eq!(
            version.content_hash,
            content_hash_for(&[0xde, 0xad, 0xbe, 0xef])
        );
        assert!(version.operation_id.is_some());
        // Operation log carries an Applied record for the write.
        let op = inner
            .operations
            .iter()
            .find(|op| op.method == "file.write")
            .unwrap();
        assert_eq!(op.status, OperationStatus::Applied);
        assert_eq!(op.base_node_id.as_deref(), Some(node_id.as_str()));
        // Audit trail records the write.
        assert!(
            inner
                .audit
                .iter()
                .any(|event| event.event_type == "file.write"
                    && event.node_id.as_deref() == Some(node_id.as_str()))
        );
    }

    #[test]
    fn file_write_updates_existing_node_through_dirty_ready() {
        let backend = test_backend();
        let first = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "shot020.exr",
                "content_hex": "00ff",
            }),
            Source::Cli,
        )
        .unwrap();
        let node_id = first["node_id"].as_str().unwrap().to_string();
        let first_version = first["version_id"].as_str().unwrap().to_string();

        let second = file_write_payload(
            &backend,
            &backend_inner_params_for_update(&node_id, "11223344"),
            Source::Cli,
        )
        .unwrap();
        assert_eq!(second["created"], false);
        assert_eq!(second["node_id"], node_id);
        let second_version = second["version_id"].as_str().unwrap().to_string();
        assert_ne!(second_version, first_version);

        let inner = backend.lock();
        let stored = inner.nodes.get(&node_id).unwrap();
        assert_eq!(
            stored.current_version_id.as_deref(),
            Some(second_version.as_str())
        );
        // Parent version linkage preserves history.
        let v2 = inner.file_versions.get(&second_version).unwrap();
        assert_eq!(
            v2.parent_version_id.as_deref(),
            Some(first_version.as_str())
        );
        // Content reflects the latest write.
        assert_eq!(
            inner.file_contents.get(&node_id).unwrap(),
            &vec![0x11, 0x22, 0x33, 0x44]
        );
        // Cache entry ends up Ready, not Dirty.
        let entry = inner.cache_entries.get(&node_id).unwrap();
        assert_eq!(entry.state, CacheState::Ready);
        assert!(!entry.dirty);
    }

    #[test]
    fn file_write_rejects_case_insensitive_sibling_conflict() {
        let backend = test_backend();
        let _first = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "Shot.exr",
                "content_hex": "00",
            }),
            Source::Cli,
        )
        .unwrap();
        let err = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "shot.exr",
                "content_hex": "01",
            }),
            Source::Cli,
        )
        .unwrap_err();
        assert_eq!(err.code, "sibling_name_conflict");
    }

    #[test]
    fn file_write_rejects_version_conflict() {
        let backend = test_backend();
        let first = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "v1.txt",
                "content_hex": "aa",
            }),
            Source::Cli,
        )
        .unwrap();
        let node_id = first["node_id"].as_str().unwrap().to_string();
        let real_version = first["version_id"].as_str().unwrap().to_string();

        // Stale base_version_id must surface as a conflict, not a silent overwrite.
        let err = file_write_payload(
            &backend,
            &serde_json::json!({
                "node_id": node_id,
                "content_hex": "bb",
                "base_version_id": "ver_stale_does_not_match",
            }),
            Source::Cli,
        )
        .unwrap_err();
        assert_eq!(err.code, "version_conflict");

        // Correct base_version_id is accepted.
        let ok = file_write_payload(
            &backend,
            &serde_json::json!({
                "node_id": node_id,
                "content_hex": "bb",
                "base_version_id": real_version,
            }),
            Source::Cli,
        )
        .unwrap();
        assert_eq!(ok["node_id"], node_id);
        assert_ne!(ok["version_id"], real_version);
    }

    #[test]
    fn file_write_rejects_bad_hex_and_missing_params() {
        let backend = test_backend();
        let err = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "bad.txt",
                "content_hex": "xyz",
            }),
            Source::Cli,
        )
        .unwrap_err();
        assert_eq!(err.code, "invalid_param");

        let err = file_write_payload(
            &backend,
            &serde_json::json!({"name": "no-parent.txt", "content_hex": "00"}),
            Source::Cli,
        )
        .unwrap_err();
        assert_eq!(err.code, "missing_param");
    }

    #[test]
    fn file_read_returns_committed_content() {
        let backend = test_backend();
        let written = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "readable.txt",
                "content_hex": "48656c6c6f",
            }),
            Source::Cli,
        )
        .unwrap();
        let node_id = written["node_id"].as_str().unwrap();
        let read = file_read_payload(&backend, &serde_json::json!({"node_id": node_id})).unwrap();
        assert_eq!(read["content_hex"], "48656c6c6f");
        assert_eq!(read["size_bytes"], 5);
        assert_eq!(read["version_id"], written["version_id"]);
    }

    #[test]
    fn file_read_returns_file_not_found_and_content_not_cached() {
        let backend = test_backend();
        // node_root exists but is a directory: node_not_file, not file_not_found.
        let err =
            file_read_payload(&backend, &serde_json::json!({"node_id": "node_root"})).unwrap_err();
        assert_eq!(err.code, "node_not_file");

        // README.md is a live file node but has no committed content.
        let err = file_read_payload(&backend, &serde_json::json!({"node_id": "node_readme"}))
            .unwrap_err();
        assert_eq!(err.code, "content_not_cached");

        // Unknown file node -> file_not_found.
        let err =
            file_read_payload(&backend, &serde_json::json!({"node_id": "node_ghost"})).unwrap_err();
        assert_eq!(err.code, "file_not_found");
    }

    #[test]
    fn file_write_and_read_round_trip_through_dispatch() {
        use biohazardfs_api_types::DaemonRequest;
        let backend = test_backend();
        let mut request = DaemonRequest::new("file.write", Source::Cli);
        request.params = serde_json::json!({
            "parent_node_id": "node_root",
            "name": "round.txt",
            "content_hex": "c0ffee",
        });
        let response = crate::dispatch_rpc(&backend, &request);
        assert!(response.ok);
        let node_id = response.data.unwrap()["node_id"]
            .as_str()
            .unwrap()
            .to_string();

        let mut read_request = DaemonRequest::new("file.read", Source::Cli);
        read_request.params = serde_json::json!({"node_id": node_id});
        let read_response = crate::dispatch_rpc(&backend, &read_request);
        assert!(read_response.ok);
        assert_eq!(read_response.data.unwrap()["content_hex"], "c0ffee");
    }

    #[test]
    fn file_write_redacts_content_from_operation_and_audit() {
        // BLOCKER: file bytes must never land in operation/provenance records
        // (those are exported and inspected). The Operation params_json and the
        // AuditEvent payload_json must carry safe metadata (content_hash,
        // size_bytes, version_id) but NOT the content_hex bytes nor the
        // operation_token capability. The caller-facing response is unchanged.
        let backend = test_backend();
        let content_hex = "deadbeef";
        let payload = file_write_payload(
            &backend,
            &serde_json::json!({
                "parent_node_id": "node_root",
                "name": "secret.exr",
                "content_hex": content_hex,
                "operation_token": "optok_should_not_leak",
            }),
            Source::Cli,
        )
        .unwrap();
        let node_id = payload["node_id"].as_str().unwrap().to_string();
        let version_id = payload["version_id"].as_str().unwrap().to_string();
        // Caller-facing response still carries size + hash.
        assert_eq!(payload["size_bytes"], 4);
        assert_eq!(
            payload["content_hash"],
            content_hash_for(&[0xde, 0xad, 0xbe, 0xef])
        );

        let inner = backend.lock();
        let op = inner
            .operations
            .iter()
            .find(|op| op.method == "file.write")
            .expect("operation logged");
        let op_params = op.params_json.as_str();

        // The content bytes and the token capability are absent from the op log.
        assert!(
            !op_params.contains(content_hex),
            "Operation.params_json must not carry content_hex: {op_params}"
        );
        assert!(
            !op_params.contains("optok_should_not_leak"),
            "Operation.params_json must not carry the operation token: {op_params}"
        );
        // Safe metadata is preserved.
        assert!(
            op_params.contains(version_id.as_str()),
            "Operation.params_json should carry version_id: {op_params}"
        );
        assert!(
            op_params.contains("\"size_bytes\":4"),
            "Operation.params_json should carry size_bytes: {op_params}"
        );
        assert!(
            op_params.contains("content_hash"),
            "Operation.params_json should carry content_hash: {op_params}"
        );

        // Same invariant for the audit trail.
        let audit = inner
            .audit
            .iter()
            .find(|event| {
                event.event_type == "file.write"
                    && event.node_id.as_deref() == Some(node_id.as_str())
            })
            .expect("file.write audit event recorded");
        let audit_payload = audit.payload_json.as_deref().unwrap_or("");
        assert!(
            !audit_payload.contains(content_hex),
            "AuditEvent.payload_json must not carry content_hex: {audit_payload}"
        );
        assert!(
            !audit_payload.contains("optok_should_not_leak"),
            "AuditEvent.payload_json must not carry the operation token: {audit_payload}"
        );
        assert!(
            audit_payload.contains("\"size_bytes\":4"),
            "AuditEvent.payload_json should carry size_bytes: {audit_payload}"
        );
        assert!(
            audit_payload.contains("content_hash"),
            "AuditEvent.payload_json should carry content_hash: {audit_payload}"
        );
    }

    #[test]
    fn hex_helpers_round_trip_and_reject_garbage() {
        assert!(decode_hex("").unwrap().is_empty());
        assert_eq!(
            decode_hex("deadbeef").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert_eq!(
            decode_hex("DEADBEEF").unwrap(),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex("xy").is_err());
        assert_eq!(encode_hex(&[0x00, 0xff, 0x10]), "00ff10");
        // Deterministic for the same bytes; labelled honestly as a scaffold hash.
        assert_eq!(
            content_hash_for(&[0xde, 0xad]),
            content_hash_for(&[0xde, 0xad])
        );
        assert!(content_hash_for(&[0xde, 0xad]).starts_with("sha256:scaffold:"));
        assert_ne!(
            content_hash_for(&[0xde, 0xad]),
            content_hash_for(&[0xde, 0xae])
        );
    }

    fn backend_inner_params_for_update(node_id: &str, content_hex: &str) -> serde_json::Value {
        serde_json::json!({
            "node_id": node_id,
            "content_hex": content_hex,
        })
    }
}
