use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod known_methods;

pub const COMMAND_SCHEMA_VERSION: &str = "2026-07-commands-v1";
pub const DAEMON_SCHEMA_VERSION: &str = "2026-07-daemon-v1";
pub const EVENT_SCHEMA_VERSION: &str = "2026-07-events-v1";
pub const SERVER_SCHEMA_VERSION: &str = "2026-07-server-v1";
pub const PRODUCT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Development/integration-only loopback endpoint for the optional HTTP transport.
///
/// Production clients should discover platform IPC from the owner-only daemon
/// descriptor file instead of treating this as the canonical daemon endpoint.
pub const DEV_LOOPBACK_HTTP_ENDPOINT: &str = "127.0.0.1:47666";
pub const DEV_LOOPBACK_RPC_PATH: &str = "/rpc";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseEnvelope<T> {
    pub ok: bool,
    pub method: String,
    pub data: Option<T>,
    pub warnings: Vec<Warning>,
    pub error: Option<ApiError>,
    pub meta: ResponseMeta,
}

impl<T> ResponseEnvelope<T>
where
    T: Serialize,
{
    pub fn ok(method: impl Into<String>, data: T, source: Source) -> Self {
        Self::ok_with_request_id(method.into(), request_id(), data, source)
    }

    pub fn ok_with_request_id(
        method: impl Into<String>,
        request_id: impl Into<String>,
        data: T,
        source: Source,
    ) -> Self {
        Self {
            ok: true,
            method: method.into(),
            data: Some(data),
            warnings: Vec::new(),
            error: None,
            meta: ResponseMeta::new(source, request_id.into()),
        }
    }

    pub fn error(method: impl Into<String>, error: ApiError, source: Source) -> Self {
        Self::error_with_request_id(method.into(), request_id(), error, source)
    }

    pub fn error_with_request_id(
        method: impl Into<String>,
        request_id: impl Into<String>,
        error: ApiError,
        source: Source,
    ) -> Self {
        Self {
            ok: false,
            method: method.into(),
            data: None,
            warnings: Vec::new(),
            error: Some(error),
            meta: ResponseMeta::new(source, request_id.into()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandResponseEnvelope<T> {
    pub ok: bool,
    pub command: String,
    pub data: Option<T>,
    pub warnings: Vec<Warning>,
    pub error: Option<ApiError>,
    pub meta: CommandResponseMeta,
}

impl<T> CommandResponseEnvelope<T>
where
    T: Serialize,
{
    pub fn ok(command: impl Into<String>, data: T, source: Source) -> Self {
        Self {
            ok: true,
            command: command.into(),
            data: Some(data),
            warnings: Vec::new(),
            error: None,
            meta: CommandResponseMeta::new(source),
        }
    }

    pub fn error(command: impl Into<String>, error: ApiError, source: Source) -> Self {
        Self {
            ok: false,
            command: command.into(),
            data: None,
            warnings: Vec::new(),
            error: Some(error),
            meta: CommandResponseMeta::new(source),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandResponseMeta {
    pub request_id: String,
    pub timestamp: String,
    pub actor: Option<ActorMeta>,
    pub device: Option<DeviceMeta>,
    pub source: Source,
    pub schema_version: String,
}

impl CommandResponseMeta {
    pub fn new(source: Source) -> Self {
        Self {
            request_id: request_id(),
            timestamp: timestamp(),
            actor: None,
            device: None,
            source,
            schema_version: COMMAND_SCHEMA_VERSION.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseMeta {
    pub request_id: String,
    pub timestamp: String,
    pub actor: Option<ActorMeta>,
    pub device: Option<DeviceMeta>,
    pub source: Source,
    pub schema_version: String,
    pub server_direct: bool,
}

impl ResponseMeta {
    pub fn new(source: Source, request_id: String) -> Self {
        Self {
            request_id,
            timestamp: timestamp(),
            actor: None,
            device: None,
            source,
            schema_version: DAEMON_SCHEMA_VERSION.to_string(),
            server_direct: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerResponseEnvelope<T> {
    pub ok: bool,
    pub operation: String,
    pub data: Option<T>,
    pub warnings: Vec<Warning>,
    pub error: Option<ApiError>,
    pub meta: ServerResponseMeta,
}

impl<T> ServerResponseEnvelope<T>
where
    T: Serialize,
{
    pub fn ok(operation: impl Into<String>, data: T, source: Source) -> Self {
        Self {
            ok: true,
            operation: operation.into(),
            data: Some(data),
            warnings: Vec::new(),
            error: None,
            meta: ServerResponseMeta::new(source),
        }
    }

    pub fn error(operation: impl Into<String>, error: ApiError, source: Source) -> Self {
        Self {
            ok: false,
            operation: operation.into(),
            data: None,
            warnings: Vec::new(),
            error: Some(error),
            meta: ServerResponseMeta::new(source),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerResponseMeta {
    pub request_id: String,
    pub timestamp: String,
    pub source: Source,
    pub schema_version: String,
    pub api_version: String,
}

impl ServerResponseMeta {
    pub fn new(source: Source) -> Self {
        Self {
            request_id: request_id(),
            timestamp: timestamp(),
            source,
            schema_version: SERVER_SCHEMA_VERSION.to_string(),
            api_version: "v1".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerStatus {
    pub name: String,
    pub version: String,
    pub state: ServerState,
    pub mode: String,
    pub api_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerState {
    Ready,
    Degraded,
    Migrating,
    WorkerReady,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerVersion {
    pub name: String,
    pub version: String,
    pub api_version: String,
    pub schema_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespaceChildrenResponse {
    pub parent_node_id: Option<String>,
    pub limit: u32,
    pub nodes: Vec<NamespaceNodeSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NamespaceNodeSummary {
    pub node_id: String,
    pub parent_node_id: Option<String>,
    pub name: String,
    pub kind: String,
    pub current_version_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentObjectPutResponse {
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_provider: String,
    pub object_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentObjectGetResponse {
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_provider: String,
    pub object_key: String,
    pub content_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContentPutResponse {
    pub node_id: String,
    pub parent_node_id: Option<String>,
    pub name: String,
    pub version_id: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_provider: String,
    pub object_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileContentGetResponse {
    pub node_id: String,
    pub parent_node_id: Option<String>,
    pub name: String,
    pub version_id: String,
    pub content_hash: String,
    pub size_bytes: u64,
    pub storage_provider: String,
    pub object_key: String,
    pub content_hex: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerHealth {
    pub state: ServerState,
    pub checks: Vec<ServerHealthCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerHealthCheck {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonRequest {
    pub id: Option<String>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub meta: DaemonRequestMeta,
}

impl DaemonRequest {
    pub fn new(method: impl Into<String>, source: Source) -> Self {
        Self {
            id: Some(request_id()),
            method: method.into(),
            params: Value::Object(Default::default()),
            meta: DaemonRequestMeta::new(source),
        }
    }

    pub fn request_id(&self) -> String {
        self.id.clone().unwrap_or_else(request_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonRequestMeta {
    pub source: Source,
    pub actor_hint: Option<String>,
    pub impersonated_user_id: Option<String>,
    pub schema_version: String,
}

impl DaemonRequestMeta {
    pub fn new(source: Source) -> Self {
        Self {
            source,
            actor_hint: None,
            impersonated_user_id: None,
            schema_version: DAEMON_SCHEMA_VERSION.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActorMeta {
    pub id: String,
    pub display_name: String,
    pub impersonated_user_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceMeta {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Ui,
    Cli,
    Agent,
    Api,
    Server,
    Test,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Warning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(
        code: impl Into<String>,
        message: impl Into<String>,
        details: Value,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(details),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientStatus {
    pub name: String,
    pub version: String,
    pub daemon_transport: String,
    pub daemon_endpoint: Option<String>,
    pub daemon_reachable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonStatus {
    pub name: String,
    pub version: String,
    pub state: DaemonState,
    pub transport: String,
    pub endpoint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    Starting,
    Ready,
    Degraded,
    Stopping,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandSchemaSummary {
    pub commands: Vec<String>,
    pub note: String,
}

// ----- Event stream envelope (DAEMON_API.md "Event stream"; EVENT_SCHEMA_VERSION) -----

/// One-way structured event carried over the daemon event stream (NDJSON over
/// IPC, SSE/NDJSON over dev loopback). The discriminator is `type` on the wire.
/// Clients must tolerate unknown event types by resyncing through state/list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventEnvelope {
    #[serde(rename = "type")]
    pub event_type: String,
    pub id: String,
    pub timestamp: String,
    pub data: Value,
    pub meta: EventMeta,
}

impl EventEnvelope {
    pub fn new(event_type: impl Into<String>, data: Value) -> Self {
        Self {
            event_type: event_type.into(),
            id: request_id(),
            timestamp: timestamp(),
            data,
            meta: EventMeta::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EventMeta {
    pub request_id: Option<String>,
    pub actor_id: Option<String>,
    pub device_id: Option<String>,
    pub schema_version: String,
}

impl Default for EventMeta {
    fn default() -> Self {
        Self {
            request_id: None,
            actor_id: None,
            device_id: None,
            schema_version: EVENT_SCHEMA_VERSION.to_string(),
        }
    }
}

/// Initial event family names (DAEMON_API.md). Stable dotted strings.
pub mod event_types {
    pub const DAEMON_STARTED: &str = "daemon.started";
    pub const DAEMON_STOPPING: &str = "daemon.stopping";
    pub const DAEMON_HEALTH_CHANGED: &str = "daemon.health_changed";
    pub const AUTH_CHANGED: &str = "auth.changed";
    pub const MOUNT_ATTACHED: &str = "mount.attached";
    pub const MOUNT_DETACHED: &str = "mount.detached";
    pub const MOUNT_HEALTH_CHANGED: &str = "mount.health_changed";
    pub const FILE_CHANGED: &str = "file.changed";
    pub const CACHE_STATE_CHANGED: &str = "cache.state_changed";
    pub const CACHE_QUOTA_WARNING: &str = "cache.quota_warning";
    pub const TRANSFER_QUEUED: &str = "transfer.queued";
    pub const TRANSFER_PROGRESS: &str = "transfer.progress";
    pub const TRANSFER_COMPLETED: &str = "transfer.completed";
    pub const TRANSFER_FAILED: &str = "transfer.failed";
    pub const LOCK_CHANGED: &str = "lock.changed";
    pub const CONFLICT_DETECTED: &str = "conflict.detected";
    pub const CONFLICT_RESOLVED: &str = "conflict.resolved";
    pub const SNAPSHOT_CREATED: &str = "snapshot.created";
    pub const SNAPSHOT_MOUNTED: &str = "snapshot.mounted";
    pub const AUDIT_EVENT_RECORDED: &str = "audit.event_recorded";
    pub const WARNING_RAISED: &str = "warning.raised";
}

// ----- Mutation safety + dry-run operation tokens (COMMANDS.md / DAEMON_API.md) -----

/// How strongly a method mutates state. Drives dry-run operation-token rules
/// under the `AgentSafe` mutation profile: destructive/admin/data-moving
/// methods require a token binding the validated params and plan hash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationClassification {
    Read,
    LowRisk,
    Destructive,
    Admin,
    DataMoving,
}

/// Fresh installs default to `AgentSafe`; first-run setup may choose `HumanFriendly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MutationProfile {
    #[default]
    AgentSafe,
    HumanFriendly,
}

/// Dry-run operation token binding validated params, actor, device, source,
/// classification, plan hash, and expiry. Applying with changed params must fail.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationToken {
    pub operation_token: String,
    pub method: String,
    pub params_hash: String,
    pub plan_hash: String,
    pub actor_id: Option<String>,
    pub device_id: Option<String>,
    pub source: Source,
    pub classification: MutationClassification,
    pub expires_at: String,
}

pub fn timestamp() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

pub fn request_id() -> String {
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    static PROCESS_PREFIX: OnceLock<String> = OnceLock::new();

    let prefix = PROCESS_PREFIX.get_or_init(|| {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        format!("{}_{}", std::process::id(), nanos)
    });
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req_{prefix}_{sequence}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_is_explicit_utc_rfc3339() {
        let timestamp = timestamp();
        assert!(timestamp.ends_with('Z'), "{timestamp}");
        assert!(timestamp.contains('T'), "{timestamp}");
    }

    #[test]
    fn daemon_request_defaults_to_contract_schema() {
        let request = DaemonRequest::new("daemon.status", Source::Cli);
        assert_eq!(request.method, "daemon.status");
        assert_eq!(request.meta.schema_version, DAEMON_SCHEMA_VERSION);
        assert_eq!(request.meta.source, Source::Cli);
    }

    #[test]
    fn request_ids_are_unique_within_process() {
        let first = request_id();
        let second = request_id();
        assert_ne!(first, second);
    }

    #[test]
    fn server_response_envelope_uses_operation_and_server_schema() {
        let envelope = ServerResponseEnvelope::ok(
            "server.status",
            ServerStatus {
                name: "biohazardfs-server".to_string(),
                version: PRODUCT_VERSION.to_string(),
                state: ServerState::Ready,
                mode: "serve".to_string(),
                api_version: "v1".to_string(),
            },
            Source::Server,
        );

        let value = serde_json::to_value(envelope).expect("envelope serializes");
        assert_eq!(value["operation"], "server.status");
        assert!(value.get("command").is_none());
        assert!(value.get("method").is_none());
        assert_eq!(value["meta"]["schema_version"], SERVER_SCHEMA_VERSION);
        assert_eq!(value["meta"]["api_version"], "v1");
    }

    #[test]
    fn command_response_envelope_uses_command_and_command_schema() {
        let envelope = CommandResponseEnvelope::ok(
            "client.status",
            ClientStatus {
                name: "biohazardfs".to_string(),
                version: PRODUCT_VERSION.to_string(),
                daemon_transport: "test".to_string(),
                daemon_endpoint: None,
                daemon_reachable: false,
            },
            Source::Cli,
        );

        let value = serde_json::to_value(envelope).expect("envelope serializes");
        assert_eq!(value["command"], "client.status");
        assert!(value.get("method").is_none());
        assert_eq!(value["meta"]["schema_version"], COMMAND_SCHEMA_VERSION);
    }

    #[test]
    fn response_envelope_uses_method_and_request_id() {
        let envelope = ResponseEnvelope::ok_with_request_id(
            "daemon.status",
            "req_test",
            DaemonStatus {
                name: "biohazardfsd".to_string(),
                version: PRODUCT_VERSION.to_string(),
                state: DaemonState::Ready,
                transport: "test".to_string(),
                endpoint: "test".to_string(),
            },
            Source::Server,
        );

        let value = serde_json::to_value(envelope).expect("envelope serializes");
        assert_eq!(value["method"], "daemon.status");
        assert!(value.get("command").is_none());
        assert_eq!(value["meta"]["request_id"], "req_test");
        assert_eq!(value["meta"]["schema_version"], DAEMON_SCHEMA_VERSION);
        assert_eq!(value["meta"]["server_direct"], false);
    }
}
