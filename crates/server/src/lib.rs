use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, AuditEventSummary, AuditEventsResponse, ConflictListResponse, ConflictSummary,
    ContentObjectGetResponse, ContentObjectPutResponse, DeviceListResponse, DeviceSummary,
    FileContentGetResponse, FileContentPutResponse, LockAcquireResponse, LockListResponse,
    LockReleaseResponse, LockSummary, NamespaceChildrenResponse, NamespaceNodeSummary,
    OperationSubmitResponse, PRODUCT_VERSION, ProjectListResponse, ProjectSummary,
    SERVER_SCHEMA_VERSION, ServerHealth, ServerHealthCheck, ServerResponseEnvelope, ServerState,
    ServerStatus, ServerVersion, Source, TrashListResponse, TrashSummary, WorksetListResponse,
    WorksetSummary, request_id,
};
use biohazardfs_core::config::RuntimeConfig;
use hmac::{Hmac, KeyInit, Mac};
use postgres::config::SslMode;
use postgres::{Client, Config, NoTls};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;

pub const DEFAULT_BIND_ADDR: &str = biohazardfs_core::config::DEFAULT_SERVER_BIND;
pub const CONTAINER_BIND_ADDR: &str = "0.0.0.0:8080";
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
const MAX_CONTENT_UPLOAD_BYTES: usize = 1024 * 1024;
const MAX_OBJECT_RESPONSE_BYTES: usize = 1024 * 1024 + 16 * 1024;
const DEFAULT_NAMESPACE_LIMIT: u32 = 100;
const MAX_NAMESPACE_LIMIT: u32 = 500;
const DEFAULT_LIST_LIMIT: u32 = 100;
const MAX_LIST_LIMIT: u32 = 500;
const DEFAULT_LOCK_TTL_SECONDS: u64 = 30 * 60;
const MAX_LOCK_TTL_SECONDS: u64 = 24 * 60 * 60;
const MAX_OPERATION_KIND_LEN: usize = 128;
const MAX_IDEMPOTENCY_KEY_LEN: usize = 256;
const DEFAULT_OBJECT_STORE_REGION: &str = "us-east-1";
const EMPTY_PAYLOAD_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const AWS4_REQUEST: &str = "aws4_request";
const S3_SERVICE: &str = "s3";
const AWS_DATE_FORMAT: &[FormatItem<'_>] = format_description!("[year][month][day]");
const AWS_DATETIME_FORMAT: &[FormatItem<'_>] =
    format_description!("[year][month][day]T[hour][minute][second]Z");
type HmacSha256 = Hmac<Sha256>;

const SCHEMA_MIGRATIONS_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_migrations (
    version TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    checksum TEXT NOT NULL,
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;
const ADVISORY_MIGRATION_LOCK_ID: i64 = 0x0042_6846_534d_5650;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationReport {
    pub name: String,
    pub mode: String,
    pub status: String,
    pub database_configured: bool,
    pub migration_count: usize,
    pub current_version: Option<String>,
    pub applied_migrations: Vec<MigrationSummary>,
    pub already_applied_migrations: Vec<MigrationSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MigrationSummary {
    pub version: String,
    pub name: String,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectStoreCheckReport {
    pub name: String,
    pub provider: String,
    pub endpoint_configured: bool,
    pub bucket: String,
    pub region: String,
    pub credentials_configured: bool,
    pub status: String,
    pub http_status: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectStoreError {
    code: &'static str,
    message: &'static str,
}

impl ObjectStoreError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self { code, message }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub fn into_api_error(self) -> ApiError {
        ApiError::new(self.code, self.message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationError {
    code: &'static str,
    message: &'static str,
    details: Option<serde_json::Value>,
}

impl MigrationError {
    fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            details: None,
        }
    }

    fn with_details(code: &'static str, message: &'static str, details: serde_json::Value) -> Self {
        Self {
            code,
            message,
            details: Some(details),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &'static str {
        self.message
    }

    pub fn into_api_error(self) -> ApiError {
        match self.details {
            Some(details) => ApiError::with_details(self.code, self.message, details),
            None => ApiError::new(self.code, self.message),
        }
    }
}

struct Migration {
    version: &'static str,
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppliedMigration {
    version: String,
    name: String,
    checksum: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuthenticatedSubject {
    org_id: String,
    user_id: String,
    scopes_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileContentPutQuery {
    parent_node_id: Option<String>,
    name: String,
    source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileContentGetQuery {
    node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileRecord {
    node_id: String,
    parent_node_id: Option<String>,
    name: String,
    version_id: String,
    content_hash: String,
    size_bytes: u64,
    storage_provider: String,
    object_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NamespaceChildrenQuery {
    parent_node_id: Option<String>,
    limit: u32,
}

pub fn server_status(mode: impl Into<String>) -> ServerStatus {
    ServerStatus {
        name: "biohazardfs-server".to_string(),
        version: PRODUCT_VERSION.to_string(),
        state: ServerState::Ready,
        mode: mode.into(),
        api_version: "v1".to_string(),
    }
}

pub fn server_version() -> ServerVersion {
    ServerVersion {
        name: "biohazardfs-server".to_string(),
        version: PRODUCT_VERSION.to_string(),
        api_version: "v1".to_string(),
        schema_version: SERVER_SCHEMA_VERSION.to_string(),
    }
}

pub fn server_health() -> ServerHealth {
    let config = RuntimeConfig::from_env();
    server_health_with_config(&config)
}

pub fn server_health_with_config(config: &RuntimeConfig) -> ServerHealth {
    ServerHealth {
        state: ServerState::Ready,
        checks: vec![
            ServerHealthCheck {
                name: "process".to_string(),
                ok: true,
                message: "server process is running".to_string(),
            },
            ServerHealthCheck {
                name: "database".to_string(),
                ok: true,
                message: if config.database.url_set {
                    "database URL is configured; migration verification is readiness-only"
                        .to_string()
                } else {
                    "database URL is not configured; liveness does not require database access"
                        .to_string()
                },
            },
            ServerHealthCheck {
                name: "object_store".to_string(),
                ok: true,
                message: format!(
                    "{} object-store config is {}; bucket check is scaffolded",
                    config.object_store.provider,
                    if config.object_store.endpoint.is_some() {
                        "present"
                    } else {
                        "missing"
                    }
                ),
            },
        ],
    }
}

pub fn server_readiness() -> ServerHealth {
    let config = RuntimeConfig::from_env();
    server_readiness_with_config(&config)
}

pub fn server_readiness_with_config(config: &RuntimeConfig) -> ServerHealth {
    let database_check = if config.database.url_set {
        match verify_latest_migration_from_config(config) {
            Ok(()) => ServerHealthCheck {
                name: "database".to_string(),
                ok: true,
                message: "database schema migrations are verified".to_string(),
            },
            Err(_) => ServerHealthCheck {
                name: "database".to_string(),
                ok: false,
                message:
                    "database schema migrations are not verified; run biohazardfs-server migrate"
                        .to_string(),
            },
        }
    } else {
        ServerHealthCheck {
            name: "database".to_string(),
            ok: true,
            message: "database URL is not configured; readiness is liveness-only".to_string(),
        }
    };

    let state = if database_check.ok {
        ServerState::Ready
    } else {
        ServerState::Degraded
    };

    ServerHealth {
        state,
        checks: vec![
            ServerHealthCheck {
                name: "process".to_string(),
                ok: true,
                message: "server process is running".to_string(),
            },
            database_check,
        ],
    }
}

pub fn migrate_payload() -> Result<MigrationReport, MigrationError> {
    let config = RuntimeConfig::from_env();
    migrate_payload_with_config(&config)
}

pub fn migrate_payload_with_config(
    config: &RuntimeConfig,
) -> Result<MigrationReport, MigrationError> {
    run_migrations_from_config(config)
}

pub fn worker_payload() -> serde_json::Value {
    serde_json::json!({
        "name": "biohazardfs-server",
        "mode": "worker",
        "status": "scaffold_ready",
        "queues": []
    })
}

/// Redacted admin status envelope payload. Exposes only booleans for database
/// and object-store configuration so the admin surface never echoes connection
/// strings or credential material. The admin subcommand does not perform admin
/// work yet; it reports readiness for an operator or agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminReport {
    pub name: String,
    pub version: String,
    pub mode: String,
    pub status: String,
    pub database_configured: bool,
    pub object_store_configured: bool,
}

pub fn admin_payload(config: &RuntimeConfig) -> AdminReport {
    AdminReport {
        name: "biohazardfs-server".to_string(),
        version: PRODUCT_VERSION.to_string(),
        mode: "admin".to_string(),
        status: "scaffold_ready".to_string(),
        database_configured: config.database.url_set,
        object_store_configured: config.object_store.endpoint.is_some(),
    }
}

pub fn object_store_check_payload_with_config(
    config: &RuntimeConfig,
) -> Result<ObjectStoreCheckReport, ObjectStoreError> {
    let request = ObjectStoreRequest::from_config(config, "HEAD")?;
    let response = send_signed_object_store_request(&request)?;
    if response.status == 200 {
        Ok(request.report("bucket_available", response.status))
    } else if response.status == 404 {
        Err(ObjectStoreError::new(
            "object_store_bucket_missing",
            "configured object-store bucket does not exist",
        ))
    } else if response.status == 403 || response.status == 401 {
        Err(ObjectStoreError::new(
            "object_store_auth_failed",
            "object-store rejected configured credentials",
        ))
    } else {
        Err(ObjectStoreError::new(
            "object_store_unavailable",
            "object-store bucket check failed",
        ))
    }
}

pub fn object_store_ensure_bucket_payload_with_config(
    config: &RuntimeConfig,
) -> Result<ObjectStoreCheckReport, ObjectStoreError> {
    match object_store_check_payload_with_config(config) {
        Ok(report) => Ok(report),
        Err(error) if error.code == "object_store_bucket_missing" => {
            let request = ObjectStoreRequest::from_config(config, "PUT")?;
            let response = send_signed_object_store_request(&request)?;
            if response.status == 200 {
                Ok(request.report("bucket_available", response.status))
            } else if response.status == 409 {
                let check_request = ObjectStoreRequest::from_config(config, "HEAD")?;
                let check_response = send_signed_object_store_request(&check_request)?;
                if check_response.status == 200 {
                    Ok(check_request.report("bucket_available", check_response.status))
                } else {
                    Err(ObjectStoreError::new(
                        "object_store_bucket_conflict",
                        "object-store bucket name is not usable by the configured credentials",
                    ))
                }
            } else if response.status == 403 || response.status == 401 {
                Err(ObjectStoreError::new(
                    "object_store_auth_failed",
                    "object-store rejected configured credentials",
                ))
            } else {
                Err(ObjectStoreError::new(
                    "object_store_unavailable",
                    "object-store bucket creation failed",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

pub fn dispatch_http_path(path: &str) -> (u16, String) {
    let config = RuntimeConfig::from_env();
    dispatch_http_path_with_config(path, &config)
}

pub fn dispatch_http_path_with_config(path: &str, config: &RuntimeConfig) -> (u16, String) {
    dispatch_http_request_with_config("GET", path, &[], &[], config)
}

fn dispatch_http_request_with_config(
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: &[u8],
    config: &RuntimeConfig,
) -> (u16, String) {
    let (route_path, query) = split_path_and_query(path);
    // handle_stream already rejects HTTP verbs outside this set; defend dispatch
    // callers (tests, embedders) the same way. Per-route method correctness is
    // enforced by the match arms and the is_known_server_route 405 fallback.
    if !matches!(method, "GET" | "PUT" | "POST" | "DELETE") {
        return method_not_allowed();
    }

    match route_path {
        "/healthz" | "/health" => json_response(
            200,
            &ServerResponseEnvelope::ok(
                "server.health",
                server_health_with_config(config),
                Source::Server,
            ),
        ),
        "/readyz" | "/ready" => {
            let readiness = server_readiness_with_config(config);
            let status_code = if readiness.state == ServerState::Ready {
                200
            } else {
                503
            };
            json_response(
                status_code,
                &ServerResponseEnvelope::ok("server.ready", readiness, Source::Server),
            )
        }
        "/version" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.version", server_version(), Source::Server),
        ),
        "/api/v1/status" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.status", server_status("serve"), Source::Server),
        ),
        // Spine: locks (acquire/list/release are backed by Postgres).
        "/api/v1/locks" if method == "GET" => locks_list_response(query, headers, config),
        "/api/v1/locks" if method == "POST" => locks_acquire_response(body, headers, config),
        "/api/v1/locks" if method == "DELETE" => locks_release_response(query, headers, config),
        // Spine: conflicts (list and show share one path; show keys on conflict_id).
        "/api/v1/conflicts" if method == "GET" => conflicts_response(query, headers, config),
        // Spine: operations.submit; replay is periphery.
        "/api/v1/operations" if method == "POST" => {
            operations_submit_response(body, headers, config)
        }
        "/api/v1/operations/replay" if method == "POST" => {
            not_implemented_response("server.operations.replay")
        }
        // Spine: trash list; restore/purge are periphery.
        "/api/v1/trash" if method == "GET" => trash_list_response(query, headers, config),
        "/api/v1/trash/restore" if method == "POST" => {
            not_implemented_response("server.trash.restore")
        }
        "/api/v1/trash/purge" if method == "POST" => not_implemented_response("server.trash.purge"),
        // Spine: audit.events; export is periphery.
        "/api/v1/audit/events" if method == "GET" => audit_events_response(query, headers, config),
        "/api/v1/audit/export" if method == "GET" => {
            not_implemented_response("server.audit.export")
        }
        // Spine: devices list; revoke is periphery.
        "/api/v1/devices" if method == "GET" => devices_list_response(query, headers, config),
        "/api/v1/devices/revoke" if method == "POST" => {
            not_implemented_response("server.devices.revoke")
        }
        // Spine: projects/worksets list; create is periphery.
        "/api/v1/projects" if method == "GET" => projects_list_response(query, headers, config),
        "/api/v1/projects" if method == "POST" => {
            not_implemented_response("server.projects.create")
        }
        "/api/v1/worksets" if method == "GET" => worksets_list_response(query, headers, config),
        "/api/v1/worksets" if method == "POST" => {
            not_implemented_response("server.worksets.create")
        }
        // Periphery: snapshots.
        "/api/v1/snapshots" if method == "GET" => not_implemented_response("server.snapshots.list"),
        "/api/v1/snapshots" if method == "POST" => {
            not_implemented_response("server.snapshots.create")
        }
        "/api/v1/snapshots/mount" if method == "POST" => {
            not_implemented_response("server.snapshots.mount")
        }
        "/api/v1/snapshots/restore" if method == "POST" => {
            not_implemented_response("server.snapshots.restore")
        }
        // Periphery: transfers.
        "/api/v1/transfers" if method == "POST" => {
            not_implemented_response("server.transfers.create")
        }
        "/api/v1/transfers/commit" if method == "POST" => {
            not_implemented_response("server.transfers.commit")
        }
        // Periphery: grants.
        "/api/v1/grants" if method == "GET" => not_implemented_response("server.grants.list"),
        "/api/v1/grants" if method == "POST" => not_implemented_response("server.grants.set"),
        "/api/v1/grants" if method == "DELETE" => not_implemented_response("server.grants.revoke"),
        // Periphery: shares.
        "/api/v1/shares" if method == "GET" => not_implemented_response("server.shares.list"),
        "/api/v1/shares" if method == "POST" => not_implemented_response("server.shares.create"),
        "/api/v1/shares" if method == "DELETE" => not_implemented_response("server.shares.revoke"),
        // Periphery: publishes.
        "/api/v1/publishes" if method == "GET" => not_implemented_response("server.publishes.list"),
        "/api/v1/publishes" if method == "POST" => {
            not_implemented_response("server.publishes.create")
        }
        "/api/v1/publishes" if method == "DELETE" => {
            not_implemented_response("server.publishes.revoke")
        }
        // Periphery: invites.
        "/api/v1/invites" if method == "GET" => not_implemented_response("server.invites.list"),
        "/api/v1/invites" if method == "POST" => not_implemented_response("server.invites.create"),
        "/api/v1/invites" if method == "DELETE" => {
            not_implemented_response("server.invites.revoke")
        }
        // Periphery: nodes.
        "/api/v1/nodes/stat" if method == "GET" => not_implemented_response("server.nodes.stat"),
        "/api/v1/nodes/mkdir" if method == "POST" => not_implemented_response("server.nodes.mkdir"),
        "/api/v1/nodes/symlink" if method == "POST" => {
            not_implemented_response("server.nodes.symlink")
        }
        "/api/v1/nodes/move" if method == "POST" => not_implemented_response("server.nodes.move"),
        "/api/v1/nodes/copy" if method == "POST" => not_implemented_response("server.nodes.copy"),
        "/api/v1/nodes" if method == "DELETE" => not_implemented_response("server.nodes.delete"),
        // Periphery: auth enrollment/login-token.
        "/api/v1/auth/device/enroll" if method == "POST" => {
            not_implemented_response("server.auth.device.enroll")
        }
        "/api/v1/auth/login_token" if method == "POST" => {
            not_implemented_response("server.auth.login_token")
        }
        "/api/v1/namespace/children" if method == "GET" => {
            namespace_children_response(query, headers, config)
        }
        "/api/v1/objects/content" if method == "PUT" => {
            content_object_put_response(headers, body, config)
        }
        "/api/v1/objects/content" if method == "GET" => {
            content_object_get_response(query, headers, config)
        }
        "/api/v1/files/content" if method == "PUT" => {
            file_content_put_response(query, headers, body, config)
        }
        "/api/v1/files/content" if method == "GET" => {
            file_content_get_response(query, headers, config)
        }
        // A known route path with an unsupported method (e.g. GET on a POST-only
        // collection) is a 405, not a 404. Unknown paths fall through to 404.
        route if is_known_server_route(route) => method_not_allowed(),
        _ => json_response(
            404,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new("not_found", "unknown server endpoint"),
                Source::Server,
            ),
        ),
    }
}

fn method_not_allowed() -> (u16, String) {
    json_response(
        405,
        &ServerResponseEnvelope::<serde_json::Value>::error(
            "server.request",
            ApiError::new(
                "method_not_allowed",
                "server endpoint does not support this method",
            ),
            Source::Server,
        ),
    )
}

fn not_implemented_response(operation: &str) -> (u16, String) {
    json_response(
        501,
        &ServerResponseEnvelope::<serde_json::Value>::error(
            operation,
            ApiError::new(
                "operation_not_implemented",
                "server operation is not implemented yet",
            ),
            Source::Server,
        ),
    )
}

fn is_known_server_route(route_path: &str) -> bool {
    matches!(
        route_path,
        "/healthz"
            | "/health"
            | "/readyz"
            | "/ready"
            | "/version"
            | "/api/v1/status"
            | "/api/v1/namespace/children"
            | "/api/v1/objects/content"
            | "/api/v1/files/content"
            | "/api/v1/locks"
            | "/api/v1/conflicts"
            | "/api/v1/operations"
            | "/api/v1/operations/replay"
            | "/api/v1/trash"
            | "/api/v1/trash/restore"
            | "/api/v1/trash/purge"
            | "/api/v1/audit/events"
            | "/api/v1/audit/export"
            | "/api/v1/devices"
            | "/api/v1/devices/revoke"
            | "/api/v1/projects"
            | "/api/v1/worksets"
            | "/api/v1/snapshots"
            | "/api/v1/snapshots/mount"
            | "/api/v1/snapshots/restore"
            | "/api/v1/transfers"
            | "/api/v1/transfers/commit"
            | "/api/v1/grants"
            | "/api/v1/shares"
            | "/api/v1/publishes"
            | "/api/v1/invites"
            | "/api/v1/nodes"
            | "/api/v1/nodes/stat"
            | "/api/v1/nodes/mkdir"
            | "/api/v1/nodes/symlink"
            | "/api/v1/nodes/move"
            | "/api/v1/nodes/copy"
            | "/api/v1/auth/device/enroll"
            | "/api/v1/auth/login_token"
    )
}

fn namespace_children_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    match namespace_children_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok("server.namespace.children", payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.namespace.children",
                error,
                Source::Server,
            ),
        ),
    }
}

fn namespace_children_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<NamespaceChildrenResponse, (u16, ApiError)> {
    let query = parse_namespace_children_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| {
        (
            401,
            ApiError::new("auth_required", "Authorization: Bearer token is required"),
        )
    })?;
    let database_url = database_url_from_config(config).map_err(|error| {
        (
            503,
            ApiError::new(
                error.code(),
                "database is not configured for namespace requests",
            ),
        )
    })?;
    let mut client = connect_database(database_url).map_err(|error| {
        (
            503,
            ApiError::new(
                error.code(),
                "database is unavailable for namespace requests",
            ),
        )
    })?;
    let subject = authenticate_subject(&mut client, bearer)?;
    list_namespace_children(&mut client, &subject, query)
}

fn file_content_put_response(
    query: &str,
    headers: &[(String, String)],
    body: &[u8],
    config: &RuntimeConfig,
) -> (u16, String) {
    match file_content_put_payload(query, headers, body, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok("server.files.content.put", payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.files.content.put",
                error,
                Source::Server,
            ),
        ),
    }
}

fn file_content_get_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    match file_content_get_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok("server.files.content.get", payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.files.content.get",
                error,
                Source::Server,
            ),
        ),
    }
}

fn content_object_put_response(
    headers: &[(String, String)],
    body: &[u8],
    config: &RuntimeConfig,
) -> (u16, String) {
    match content_object_put_payload(headers, body, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok("server.objects.content.put", payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.objects.content.put",
                error,
                Source::Server,
            ),
        ),
    }
}

fn content_object_get_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    match content_object_get_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok("server.objects.content.get", payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.objects.content.get",
                error,
                Source::Server,
            ),
        ),
    }
}

// ===== Wave 2 spine routes =====
// locks (acquire/list/release), conflicts (list/show), operations.submit,
// trash.list, audit.events, devices.list, projects.list, worksets.list are
// backed by Postgres following the record_file_content transactional template.
// Each route resolves authenticate_subject + a scopes_allow_* helper, then runs
// org-scoped SQL. Timestamps are formatted as explicit RFC3339 UTC in SQL so the
// synchronous postgres driver does not need a timestamp feature gate.

/// SQL projection fragment that formats a timestamptz column as RFC3339 UTC.
/// `column` must be a hardcoded SQL identifier, never user input.
fn ts_utc(column: &str) -> String {
    format!("to_char({column} AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"') AS {column}")
}

fn auth_required_error() -> ApiError {
    ApiError::new("auth_required", "Authorization: Bearer token is required")
}

fn connect_for_payload(config: &RuntimeConfig, purpose: &str) -> Result<Client, (u16, ApiError)> {
    let database_url = database_url_from_config(config).map_err(|error| {
        (
            503,
            db_unavailable_error(error.code(), purpose, "not configured"),
        )
    })?;
    connect_database(database_url).map_err(|error| {
        (
            503,
            db_unavailable_error(error.code(), purpose, "unavailable"),
        )
    })
}

fn db_unavailable_error(code: &str, purpose: &str, state: &str) -> ApiError {
    let message = if state == "not configured" {
        format!("database is not configured for {purpose}")
    } else {
        format!("database is unavailable for {purpose}")
    };
    ApiError::new(code, message)
}

fn parse_limit_value(value: &str) -> Result<u32, (u16, ApiError)> {
    let parsed = value.parse::<u32>().map_err(|_| {
        (
            400,
            ApiError::new("invalid_limit", "limit must be a positive integer"),
        )
    })?;
    if parsed == 0 || parsed > MAX_LIST_LIMIT {
        return Err((
            400,
            ApiError::new(
                "invalid_limit",
                format!("limit must be between 1 and {MAX_LIST_LIMIT}"),
            ),
        ));
    }
    Ok(parsed)
}

fn validate_opaque_id(value: &str, code: &str, label: &str) -> Result<(), (u16, ApiError)> {
    let valid = !value.trim().is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'));
    if valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new(code, format!("{label} is not valid for this operation")),
        ))
    }
}

fn validate_status_filter(status: &str, allowed: &[&str]) -> Result<(), (u16, ApiError)> {
    if allowed.contains(&status) {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new(
                "invalid_status",
                "status filter is not a known status value",
            ),
        ))
    }
}

fn parse_json_body<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, (u16, ApiError)> {
    if body.is_empty() {
        return Err((
            400,
            ApiError::new("body_required", "JSON request body is required"),
        ));
    }
    serde_json::from_slice(body).map_err(|_| {
        (
            400,
            ApiError::new(
                "invalid_json",
                "request body is not valid JSON for this operation",
            ),
        )
    })
}

// --- existence preflight: gives clean 404s for client-supplied IDs before the
// insert/update path would otherwise surface a FK violation as a 503. ---
fn ensure_node_exists(
    client: &mut Client,
    org_id: &str,
    node_id: &str,
) -> Result<(), (u16, ApiError)> {
    let rows = client
        .query(
            "SELECT 1 FROM nodes WHERE org_id = $1 AND node_id = $2 AND deleted_at IS NULL",
            &[&org_id, &node_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("metadata_store_unavailable", "could not verify node"),
            )
        })?;
    if rows.is_empty() {
        Err((
            404,
            ApiError::new("node_not_found", "node was not found in this organization"),
        ))
    } else {
        Ok(())
    }
}

fn ensure_file_version_exists(
    client: &mut Client,
    org_id: &str,
    version_id: &str,
) -> Result<(), (u16, ApiError)> {
    let rows = client
        .query(
            "SELECT 1 FROM file_versions WHERE org_id = $1 AND version_id = $2",
            &[&org_id, &version_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new(
                    "metadata_store_unavailable",
                    "could not verify file version",
                ),
            )
        })?;
    if rows.is_empty() {
        Err((
            404,
            ApiError::new("version_not_found", "file version was not found"),
        ))
    } else {
        Ok(())
    }
}

fn ensure_device_exists(
    client: &mut Client,
    org_id: &str,
    device_id: &str,
) -> Result<(), (u16, ApiError)> {
    let rows = client
        .query(
            "SELECT 1 FROM devices WHERE org_id = $1 AND device_id = $2",
            &[&org_id, &device_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("metadata_store_unavailable", "could not verify device"),
            )
        })?;
    if rows.is_empty() {
        Err((
            404,
            ApiError::new("device_not_found", "device was not found"),
        ))
    } else {
        Ok(())
    }
}

// ===== Locks =====

fn locks_list_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.locks.list";
    match locks_list_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

fn locks_acquire_response(
    body: &[u8],
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.locks.acquire";
    match locks_acquire_payload(body, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

fn locks_release_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.locks.release";
    match locks_release_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LockListQuery {
    node_id: Option<String>,
    status: Option<String>,
    limit: u32,
}

fn parse_lock_list_query(query: &str) -> Result<LockListQuery, (u16, ApiError)> {
    let mut node_id = None;
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "node" | "node_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_node_id_value(&decoded)?;
                node_id = Some(decoded);
            }
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(&decoded, &["active", "released", "expired", "broken"])?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(LockListQuery {
        node_id,
        status,
        limit,
    })
}

fn parse_lock_id_query(query: &str) -> Result<String, (u16, ApiError)> {
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if (key == "lock" || key == "lock_id") && !value.trim().is_empty() {
            let decoded = percent_decode_query_value(value)?;
            validate_opaque_id(&decoded, "invalid_lock_id", "lock_id")?;
            return Ok(decoded);
        }
    }
    Err((
        400,
        ApiError::new("lock_id_required", "lock_id query parameter is required"),
    ))
}

#[derive(Debug, Clone, Deserialize)]
struct LockAcquireBody {
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    provisional_local_id: Option<String>,
    #[serde(default)]
    path_snapshot: Option<String>,
    #[serde(default)]
    owner_device_id: Option<String>,
    #[serde(default = "default_lock_kind")]
    kind: String,
    #[serde(default = "default_lock_ttl_seconds")]
    ttl_seconds: u64,
}

fn default_lock_kind() -> String {
    "edit".to_string()
}

fn default_lock_ttl_seconds() -> u64 {
    DEFAULT_LOCK_TTL_SECONDS
}

fn validate_lock_kind(kind: &str) -> Result<(), (u16, ApiError)> {
    if matches!(kind, "edit" | "admin" | "publish" | "restore") {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new(
                "invalid_lock_kind",
                "lock kind must be edit, admin, publish, or restore",
            ),
        ))
    }
}

fn lock_ttl_seconds(ttl_seconds: u64) -> Result<f64, (u16, ApiError)> {
    if ttl_seconds > MAX_LOCK_TTL_SECONDS {
        return Err((
            400,
            ApiError::new(
                "invalid_lock_ttl",
                format!("lock ttl_seconds must be at most {MAX_LOCK_TTL_SECONDS}"),
            ),
        ));
    }
    Ok(ttl_seconds as f64)
}

fn locks_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<LockListResponse, (u16, ApiError)> {
    let query_params = parse_lock_list_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "lock requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_lock_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read locks"),
        ));
    }
    let node_filter = query_params.node_id.as_deref();
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let acquired = ts_utc("acquired_at");
    let expires = ts_utc("expires_at");
    let released = ts_utc("released_at");
    let rows = client
        .query(
            &format!(
                "SELECT lock_id, node_id, provisional_local_id, path_snapshot, owner_user_id,
                        owner_device_id, kind, status, {acquired}, {expires}, {released}
                 FROM locks
                 WHERE org_id = $1
                   AND ($2::text IS NULL OR node_id = $2)
                   AND ($3::text IS NULL OR status = $3)
                 ORDER BY acquired_at DESC
                 LIMIT $4"
            ),
            &[&subject.org_id, &node_filter, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("lock_store_unavailable", "could not list locks"),
            )
        })?;
    let locks = rows
        .into_iter()
        .map(|row| LockSummary {
            lock_id: row.get("lock_id"),
            node_id: row.get("node_id"),
            provisional_local_id: row.get("provisional_local_id"),
            path_snapshot: row.get("path_snapshot"),
            owner_user_id: row.get("owner_user_id"),
            owner_device_id: row.get("owner_device_id"),
            kind: row.get("kind"),
            status: row.get("status"),
            acquired_at: row.get("acquired_at"),
            expires_at: row.get("expires_at"),
            released_at: row.get("released_at"),
        })
        .collect();
    Ok(LockListResponse {
        locks,
        limit: query_params.limit,
    })
}

fn locks_acquire_payload(
    body: &[u8],
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<LockAcquireResponse, (u16, ApiError)> {
    let body = parse_json_body::<LockAcquireBody>(body)?;
    validate_lock_kind(&body.kind)?;
    let ttl_seconds = lock_ttl_seconds(body.ttl_seconds)?;
    if let Some(node_id) = body.node_id.as_deref() {
        validate_node_id_value(node_id)?;
    }
    if let Some(device_id) = body.owner_device_id.as_deref() {
        validate_opaque_id(device_id, "invalid_device_id", "owner_device_id")?;
    }
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "lock requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_lock_write(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot write locks"),
        ));
    }
    if let Some(node_id) = body.node_id.as_deref() {
        ensure_node_exists(&mut client, &subject.org_id, node_id)?;
    }
    if let Some(device_id) = body.owner_device_id.as_deref() {
        ensure_device_exists(&mut client, &subject.org_id, device_id)?;
    }
    let lock_id = generated_id("lock");
    let acquired = ts_utc("acquired_at");
    let expires = ts_utc("expires_at");
    let row = client
        .query_one(
            &format!(
                "INSERT INTO locks
                   (org_id, lock_id, node_id, provisional_local_id, path_snapshot,
                    owner_user_id, owner_device_id, kind, status, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'active',
                         now() + make_interval(secs => $9))
                 RETURNING {acquired}, {expires}"
            ),
            &[
                &subject.org_id,
                &lock_id,
                &body.node_id,
                &body.provisional_local_id,
                &body.path_snapshot,
                &subject.user_id,
                &body.owner_device_id,
                &body.kind,
                &ttl_seconds,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("lock_store_unavailable", "could not acquire lock"),
            )
        })?;
    Ok(LockAcquireResponse {
        lock_id,
        node_id: body.node_id,
        kind: body.kind,
        status: "active".to_string(),
        owner_user_id: subject.user_id,
        acquired_at: row.get("acquired_at"),
        expires_at: row.get("expires_at"),
    })
}

fn locks_release_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<LockReleaseResponse, (u16, ApiError)> {
    let lock_id = parse_lock_id_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "lock requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_lock_write(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot release locks"),
        ));
    }
    let updated = client
        .execute(
            "UPDATE locks
             SET status = 'released', released_at = now()
             WHERE org_id = $1 AND lock_id = $2 AND status = 'active'",
            &[&subject.org_id, &lock_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("lock_store_unavailable", "could not release lock"),
            )
        })?;
    if updated == 0 {
        return Err((
            404,
            ApiError::new(
                "lock_not_found",
                "active lock was not found for this organization",
            ),
        ));
    }
    Ok(LockReleaseResponse {
        lock_id,
        status: "released".to_string(),
    })
}

// ===== Conflicts =====

fn conflicts_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    if has_conflict_id(query) {
        let operation = "server.conflicts.show";
        match conflict_show_payload(query, headers, config) {
            Ok(payload) => json_response(
                200,
                &ServerResponseEnvelope::ok(operation, payload, Source::Server),
            ),
            Err((status_code, error)) => json_response(
                status_code,
                &ServerResponseEnvelope::<serde_json::Value>::error(
                    operation,
                    error,
                    Source::Server,
                ),
            ),
        }
    } else {
        let operation = "server.conflicts.list";
        match conflicts_list_payload(query, headers, config) {
            Ok(payload) => json_response(
                200,
                &ServerResponseEnvelope::ok(operation, payload, Source::Server),
            ),
            Err((status_code, error)) => json_response(
                status_code,
                &ServerResponseEnvelope::<serde_json::Value>::error(
                    operation,
                    error,
                    Source::Server,
                ),
            ),
        }
    }
}

fn has_conflict_id(query: &str) -> bool {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .any(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (key == "conflict" || key == "conflict_id") && !value.trim().is_empty()
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConflictListQuery {
    node_id: Option<String>,
    status: Option<String>,
    limit: u32,
}

fn parse_conflict_list_query(query: &str) -> Result<ConflictListQuery, (u16, ApiError)> {
    let mut node_id = None;
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "node" | "node_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_node_id_value(&decoded)?;
                node_id = Some(decoded);
            }
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(
                    &decoded,
                    &["open", "resolved", "preserved_all", "dismissed"],
                )?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(ConflictListQuery {
        node_id,
        status,
        limit,
    })
}

fn parse_conflict_id_query(query: &str) -> Result<String, (u16, ApiError)> {
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if (key == "conflict" || key == "conflict_id") && !value.trim().is_empty() {
            let decoded = percent_decode_query_value(value)?;
            validate_opaque_id(&decoded, "invalid_conflict_id", "conflict_id")?;
            return Ok(decoded);
        }
    }
    Err((
        400,
        ApiError::new(
            "conflict_id_required",
            "conflict_id query parameter is required",
        ),
    ))
}

fn conflicts_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<ConflictListResponse, (u16, ApiError)> {
    let query_params = parse_conflict_list_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "conflict requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_conflict_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read conflicts"),
        ));
    }
    let node_filter = query_params.node_id.as_deref();
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let created = ts_utc("created_at");
    let resolved = ts_utc("resolved_at");
    let rows = client
        .query(
            &format!(
                "SELECT conflict_id, node_id, path_snapshot, kind, status, base_version_id,
                        local_version_id, remote_version_id, local_operation_id,
                        remote_operation_id, {created}, {resolved}
                 FROM conflicts
                 WHERE org_id = $1
                   AND ($2::text IS NULL OR node_id = $2)
                   AND ($3::text IS NULL OR status = $3)
                 ORDER BY created_at DESC
                 LIMIT $4"
            ),
            &[&subject.org_id, &node_filter, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("conflict_store_unavailable", "could not list conflicts"),
            )
        })?;
    let conflicts = rows
        .into_iter()
        .map(|row| ConflictSummary {
            conflict_id: row.get("conflict_id"),
            node_id: row.get("node_id"),
            path_snapshot: row.get("path_snapshot"),
            kind: row.get("kind"),
            status: row.get("status"),
            base_version_id: row.get("base_version_id"),
            local_version_id: row.get("local_version_id"),
            remote_version_id: row.get("remote_version_id"),
            local_operation_id: row.get("local_operation_id"),
            remote_operation_id: row.get("remote_operation_id"),
            created_at: row.get("created_at"),
            resolved_at: row.get("resolved_at"),
        })
        .collect();
    Ok(ConflictListResponse {
        conflicts,
        limit: query_params.limit,
    })
}

fn conflict_show_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<ConflictSummary, (u16, ApiError)> {
    let conflict_id = parse_conflict_id_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "conflict requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_conflict_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read conflicts"),
        ));
    }
    let created = ts_utc("created_at");
    let resolved = ts_utc("resolved_at");
    let row = client
        .query_opt(
            &format!(
                "SELECT conflict_id, node_id, path_snapshot, kind, status, base_version_id,
                        local_version_id, remote_version_id, local_operation_id,
                        remote_operation_id, {created}, {resolved}
                 FROM conflicts
                 WHERE org_id = $1 AND conflict_id = $2"
            ),
            &[&subject.org_id, &conflict_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("conflict_store_unavailable", "could not read conflict"),
            )
        })?;
    let Some(row) = row else {
        return Err((
            404,
            ApiError::new(
                "conflict_not_found",
                "conflict was not found in this organization",
            ),
        ));
    };
    Ok(ConflictSummary {
        conflict_id: row.get("conflict_id"),
        node_id: row.get("node_id"),
        path_snapshot: row.get("path_snapshot"),
        kind: row.get("kind"),
        status: row.get("status"),
        base_version_id: row.get("base_version_id"),
        local_version_id: row.get("local_version_id"),
        remote_version_id: row.get("remote_version_id"),
        local_operation_id: row.get("local_operation_id"),
        remote_operation_id: row.get("remote_operation_id"),
        created_at: row.get("created_at"),
        resolved_at: row.get("resolved_at"),
    })
}

// ===== Operations =====

fn operations_submit_response(
    body: &[u8],
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.operations.submit";
    match operations_submit_payload(body, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, Deserialize)]
struct OperationSubmitBody {
    kind: String,
    #[serde(default = "default_operation_source")]
    source: String,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    base_version_id: Option<String>,
    #[serde(default)]
    idempotency_key: Option<String>,
    #[serde(default)]
    params: serde_json::Value,
}

fn default_operation_source() -> String {
    "api".to_string()
}

fn validate_operation_kind(kind: &str) -> Result<(), (u16, ApiError)> {
    let valid = !kind.trim().is_empty()
        && kind.len() <= MAX_OPERATION_KIND_LEN
        && kind
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b':'));
    if valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new("invalid_operation_kind", "operation kind is not valid"),
        ))
    }
}

fn validate_idempotency_key(key: &str) -> Result<(), (u16, ApiError)> {
    let valid = !key.is_empty()
        && key.len() <= MAX_IDEMPOTENCY_KEY_LEN
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'));
    if valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new("invalid_idempotency_key", "idempotency_key is not valid"),
        ))
    }
}

fn operations_submit_payload(
    body: &[u8],
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<OperationSubmitResponse, (u16, ApiError)> {
    let body = parse_json_body::<OperationSubmitBody>(body)?;
    validate_operation_kind(&body.kind)?;
    validate_file_source(&body.source)?;
    if let Some(node_id) = body.node_id.as_deref() {
        validate_node_id_value(node_id)?;
    }
    if let Some(version_id) = body.base_version_id.as_deref() {
        validate_opaque_id(version_id, "invalid_version_id", "base_version_id")?;
    }
    if let Some(device_id) = body.device_id.as_deref() {
        validate_opaque_id(device_id, "invalid_device_id", "device_id")?;
    }
    if let Some(key) = body.idempotency_key.as_deref() {
        validate_idempotency_key(key)?;
    }
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "operation submissions")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_operation_write(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot submit operations",
            ),
        ));
    }
    if let Some(node_id) = body.node_id.as_deref() {
        ensure_node_exists(&mut client, &subject.org_id, node_id)?;
    }
    if let Some(version_id) = body.base_version_id.as_deref() {
        ensure_file_version_exists(&mut client, &subject.org_id, version_id)?;
    }
    if let Some(device_id) = body.device_id.as_deref() {
        ensure_device_exists(&mut client, &subject.org_id, device_id)?;
    }
    // Idempotent replay: a resubmitted idempotency key must return the prior
    // recorded operation rather than create a duplicate or silently fail.
    if let Some(key) = body.idempotency_key.as_deref()
        && let Some(existing) = lookup_operation_by_idempotency(&mut client, &subject.org_id, key)?
    {
        return Ok(existing);
    }
    let operation_id = generated_id("op");
    let req_id = request_id();
    // The synchronous postgres driver has no serde_json feature gate, so the
    // params object is serialized to text in Rust, sent as a TEXT parameter
    // ($11::text), then cast to jsonb server-side. Binding the String directly
    // to a jsonb-typed parameter would fail at serialize time because String's
    // ToSql impl does not accept the JSONB OID.
    let params_json = serde_json::to_string(&body.params).unwrap_or_else(|_| "{}".to_string());
    let received = ts_utc("created_at");
    let row = client
        .query_one(
            &format!(
                "INSERT INTO operations
                   (org_id, operation_id, actor_user_id, device_id, source, kind, status,
                    base_version_id, node_id, idempotency_key, request_id, payload_json)
                 VALUES ($1, $2, $3, $4, $5, $6, 'pending', $7, $8, $9, $10, $11::text::jsonb)
                 RETURNING status, idempotency_key, {received}"
            ),
            &[
                &subject.org_id,
                &operation_id,
                &subject.user_id,
                &body.device_id,
                &body.source,
                &body.kind,
                &body.base_version_id,
                &body.node_id,
                &body.idempotency_key,
                &req_id,
                &params_json,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("operation_store_unavailable", "could not record operation"),
            )
        })?;
    let status: String = row.get("status");
    let idempotency_key: Option<String> = row.get("idempotency_key");
    let received_at: String = row.get("created_at");
    Ok(OperationSubmitResponse {
        operation_id,
        status,
        idempotency_key,
        received_at,
    })
}

fn lookup_operation_by_idempotency(
    client: &mut Client,
    org_id: &str,
    key: &str,
) -> Result<Option<OperationSubmitResponse>, (u16, ApiError)> {
    let created = ts_utc("created_at");
    let row = client
        .query_opt(
            &format!(
                "SELECT operation_id, status, idempotency_key, {created}
                 FROM operations
                 WHERE org_id = $1 AND idempotency_key = $2
                 LIMIT 1"
            ),
            &[&org_id, &key],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("operation_store_unavailable", "could not look up operation"),
            )
        })?;
    Ok(row.map(|row| OperationSubmitResponse {
        operation_id: row.get("operation_id"),
        status: row.get("status"),
        idempotency_key: row.get("idempotency_key"),
        received_at: row.get("created_at"),
    }))
}

// ===== Trash =====

fn trash_list_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.trash.list";
    match trash_list_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrashListQuery {
    status: Option<String>,
    limit: u32,
}

fn parse_trash_list_query(query: &str) -> Result<TrashListQuery, (u16, ApiError)> {
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(&decoded, &["trashed", "restored", "purged"])?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(TrashListQuery { status, limit })
}

fn trash_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<TrashListResponse, (u16, ApiError)> {
    let query_params = parse_trash_list_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "trash requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_trash_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read trash"),
        ));
    }
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let deleted = ts_utc("deleted_at");
    let purge_after = ts_utc("purge_after");
    let rows = client
        .query(
            &format!(
                "SELECT trash_id, node_id, original_parent_node_id, original_name,
                        deleted_version_id, deleted_by, status, {deleted}, {purge_after}
                 FROM trash_records
                 WHERE org_id = $1 AND ($2::text IS NULL OR status = $2)
                 ORDER BY deleted_at DESC
                 LIMIT $3"
            ),
            &[&subject.org_id, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("trash_store_unavailable", "could not list trash"),
            )
        })?;
    let trash = rows
        .into_iter()
        .map(|row| TrashSummary {
            trash_id: row.get("trash_id"),
            node_id: row.get("node_id"),
            original_parent_node_id: row.get("original_parent_node_id"),
            original_name: row.get("original_name"),
            deleted_version_id: row.get("deleted_version_id"),
            deleted_by: row.get("deleted_by"),
            status: row.get("status"),
            deleted_at: row.get("deleted_at"),
            purge_after: row.get("purge_after"),
        })
        .collect();
    Ok(TrashListResponse {
        trash,
        limit: query_params.limit,
    })
}

// ===== Audit events =====

fn audit_events_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.audit.events";
    match audit_events_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditEventsQuery {
    event_type: Option<String>,
    actor_user_id: Option<String>,
    source: Option<String>,
    limit: u32,
}

fn parse_audit_events_query(query: &str) -> Result<AuditEventsQuery, (u16, ApiError)> {
    let mut event_type = None;
    let mut actor_user_id = None;
    let mut source = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "event_type" | "type" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_event_type(&decoded)?;
                event_type = Some(decoded);
            }
            "actor" | "actor_user_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_opaque_id(&decoded, "invalid_actor", "actor_user_id")?;
                actor_user_id = Some(decoded);
            }
            "source" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_file_source(&decoded)?;
                source = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(AuditEventsQuery {
        event_type,
        actor_user_id,
        source,
        limit,
    })
}

fn validate_event_type(event_type: &str) -> Result<(), (u16, ApiError)> {
    let valid = event_type.len() <= 128
        && event_type
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new("invalid_event_type", "event_type is not valid"),
        ))
    }
}

fn audit_events_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<AuditEventsResponse, (u16, ApiError)> {
    let query_params = parse_audit_events_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "audit requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_audit_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot read audit events",
            ),
        ));
    }
    let event_type_filter = query_params.event_type.as_deref();
    let actor_filter = query_params.actor_user_id.as_deref();
    let source_filter = query_params.source.as_deref();
    let limit = i64::from(query_params.limit);
    let occurred = ts_utc("occurred_at");
    let rows = client
        .query(
            &format!(
                "SELECT audit_event_id, event_type, actor_user_id, device_id, source, node_id,
                        operation_id, request_id, {occurred}
                 FROM audit_events
                 WHERE org_id = $1
                   AND ($2::text IS NULL OR event_type = $2)
                   AND ($3::text IS NULL OR actor_user_id = $3)
                   AND ($4::text IS NULL OR source = $4)
                 ORDER BY occurred_at DESC
                 LIMIT $5"
            ),
            &[
                &subject.org_id,
                &event_type_filter,
                &actor_filter,
                &source_filter,
                &limit,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("audit_store_unavailable", "could not list audit events"),
            )
        })?;
    let events = rows
        .into_iter()
        .map(|row| AuditEventSummary {
            audit_event_id: row.get("audit_event_id"),
            event_type: row.get("event_type"),
            actor_user_id: row.get("actor_user_id"),
            device_id: row.get("device_id"),
            source: row.get("source"),
            node_id: row.get("node_id"),
            operation_id: row.get("operation_id"),
            request_id: row.get("request_id"),
            occurred_at: row.get("occurred_at"),
        })
        .collect();
    Ok(AuditEventsResponse {
        events,
        limit: query_params.limit,
    })
}

// ===== Devices =====

fn devices_list_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.devices.list";
    match devices_list_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceListQuery {
    user_id: Option<String>,
    status: Option<String>,
    limit: u32,
}

fn parse_device_list_query(query: &str) -> Result<DeviceListQuery, (u16, ApiError)> {
    let mut user_id = None;
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "user" | "user_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_opaque_id(&decoded, "invalid_user_id", "user_id")?;
                user_id = Some(decoded);
            }
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(&decoded, &["active", "revoked", "lost"])?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(DeviceListQuery {
        user_id,
        status,
        limit,
    })
}

fn devices_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<DeviceListResponse, (u16, ApiError)> {
    let query_params = parse_device_list_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "device requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_devices_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read devices"),
        ));
    }
    let user_filter = query_params.user_id.as_deref();
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let enrolled = ts_utc("enrolled_at");
    let last_seen = ts_utc("last_seen_at");
    let revoked = ts_utc("revoked_at");
    let rows = client
        .query(
            &format!(
                "SELECT device_id, user_id, display_name, platform, hostname, status,
                        {enrolled}, {last_seen}, {revoked}
                 FROM devices
                 WHERE org_id = $1
                   AND ($2::text IS NULL OR user_id = $2)
                   AND ($3::text IS NULL OR status = $3)
                 ORDER BY enrolled_at DESC
                 LIMIT $4"
            ),
            &[&subject.org_id, &user_filter, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("device_store_unavailable", "could not list devices"),
            )
        })?;
    let devices = rows
        .into_iter()
        .map(|row| DeviceSummary {
            device_id: row.get("device_id"),
            user_id: row.get("user_id"),
            display_name: row.get("display_name"),
            platform: row.get("platform"),
            hostname: row.get("hostname"),
            status: row.get("status"),
            enrolled_at: row.get("enrolled_at"),
            last_seen_at: row.get("last_seen_at"),
            revoked_at: row.get("revoked_at"),
        })
        .collect();
    Ok(DeviceListResponse {
        devices,
        limit: query_params.limit,
    })
}

// ===== Projects =====

fn projects_list_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.projects.list";
    match projects_list_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SimpleStatusListQuery {
    status: Option<String>,
    limit: u32,
}

fn parse_simple_status_list_query(
    query: &str,
    allowed_statuses: &[&str],
) -> Result<SimpleStatusListQuery, (u16, ApiError)> {
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(&decoded, allowed_statuses)?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(SimpleStatusListQuery { status, limit })
}

fn projects_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<ProjectListResponse, (u16, ApiError)> {
    let query_params = parse_simple_status_list_query(query, &["active", "archived"])?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "project requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_projects_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read projects"),
        ));
    }
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let created = ts_utc("created_at");
    let updated = ts_utc("updated_at");
    let rows = client
        .query(
            &format!(
                "SELECT project_id, root_node_id, name, code, status, {created}, {updated}
                 FROM projects
                 WHERE org_id = $1 AND ($2::text IS NULL OR status = $2)
                 ORDER BY created_at DESC
                 LIMIT $3"
            ),
            &[&subject.org_id, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("project_store_unavailable", "could not list projects"),
            )
        })?;
    let projects = rows
        .into_iter()
        .map(|row| ProjectSummary {
            project_id: row.get("project_id"),
            root_node_id: row.get("root_node_id"),
            name: row.get("name"),
            code: row.get("code"),
            status: row.get("status"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
        .collect();
    Ok(ProjectListResponse {
        projects,
        limit: query_params.limit,
    })
}

// ===== Worksets =====

fn worksets_list_response(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> (u16, String) {
    let operation = "server.worksets.list";
    match worksets_list_payload(query, headers, config) {
        Ok(payload) => json_response(
            200,
            &ServerResponseEnvelope::ok(operation, payload, Source::Server),
        ),
        Err((status_code, error)) => json_response(
            status_code,
            &ServerResponseEnvelope::<serde_json::Value>::error(operation, error, Source::Server),
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorksetListQuery {
    project_id: Option<String>,
    status: Option<String>,
    limit: u32,
}

fn parse_workset_list_query(query: &str) -> Result<WorksetListQuery, (u16, ApiError)> {
    let mut project_id = None;
    let mut status = None;
    let mut limit = DEFAULT_LIST_LIMIT;
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "project" | "project_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_opaque_id(&decoded, "invalid_project_id", "project_id")?;
                project_id = Some(decoded);
            }
            "status" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_status_filter(&decoded, &["active", "archived"])?;
                status = Some(decoded);
            }
            "limit" if !value.trim().is_empty() => {
                limit = parse_limit_value(value.trim())?;
            }
            _ => {}
        }
    }
    Ok(WorksetListQuery {
        project_id,
        status,
        limit,
    })
}

fn worksets_list_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<WorksetListResponse, (u16, ApiError)> {
    let query_params = parse_workset_list_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| (401, auth_required_error()))?;
    let mut client = connect_for_payload(config, "workset requests")?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_worksets_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new("auth_scope_missing", "bearer token cannot read worksets"),
        ));
    }
    let project_filter = query_params.project_id.as_deref();
    let status_filter = query_params.status.as_deref();
    let limit = i64::from(query_params.limit);
    let created = ts_utc("created_at");
    let updated = ts_utc("updated_at");
    let rows = client
        .query(
            &format!(
                "SELECT workset_id, project_id, name, description, status, source, created_by,
                        {created}, {updated}
                 FROM worksets
                 WHERE org_id = $1
                   AND ($2::text IS NULL OR project_id = $2)
                   AND ($3::text IS NULL OR status = $3)
                 ORDER BY created_at DESC
                 LIMIT $4"
            ),
            &[&subject.org_id, &project_filter, &status_filter, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("workset_store_unavailable", "could not list worksets"),
            )
        })?;
    let worksets = rows
        .into_iter()
        .map(|row| WorksetSummary {
            workset_id: row.get("workset_id"),
            project_id: row.get("project_id"),
            name: row.get("name"),
            description: row.get("description"),
            status: row.get("status"),
            source: row.get("source"),
            created_by: row.get("created_by"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
        .collect();
    Ok(WorksetListResponse {
        worksets,
        limit: query_params.limit,
    })
}

fn file_content_put_payload(
    query: &str,
    headers: &[(String, String)],
    body: &[u8],
    config: &RuntimeConfig,
) -> Result<FileContentPutResponse, (u16, ApiError)> {
    let query = parse_file_content_put_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| {
        (
            401,
            ApiError::new("auth_required", "Authorization: Bearer token is required"),
        )
    })?;
    let database_url = database_url_from_config(config).map_err(|error| {
        (
            503,
            ApiError::new(error.code(), "database is not configured for file requests"),
        )
    })?;
    let mut client = connect_database(database_url).map_err(|error| {
        (
            503,
            ApiError::new(error.code(), "database is unavailable for file requests"),
        )
    })?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_file_write(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot write file metadata",
            ),
        ));
    }
    preflight_file_content_write(&mut client, &subject, &query)?;

    let stored = store_content_object(config, &subject.org_id, body)?;
    let record = record_file_content(&mut client, &subject, query, &stored)?;
    Ok(FileContentPutResponse {
        node_id: record.node_id,
        parent_node_id: record.parent_node_id,
        name: record.name,
        version_id: record.version_id,
        content_hash: record.content_hash,
        size_bytes: record.size_bytes,
        storage_provider: record.storage_provider,
        object_key: record.object_key,
    })
}

fn file_content_get_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<FileContentGetResponse, (u16, ApiError)> {
    let query = parse_file_content_get_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| {
        (
            401,
            ApiError::new("auth_required", "Authorization: Bearer token is required"),
        )
    })?;
    let database_url = database_url_from_config(config).map_err(|error| {
        (
            503,
            ApiError::new(error.code(), "database is not configured for file requests"),
        )
    })?;
    let mut client = connect_database(database_url).map_err(|error| {
        (
            503,
            ApiError::new(error.code(), "database is unavailable for file requests"),
        )
    })?;
    let subject = authenticate_subject(&mut client, bearer)?;
    if !scopes_allow_file_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot read file metadata",
            ),
        ));
    }
    let record = load_file_record(&mut client, &subject, &query.node_id)?;
    let content = fetch_content_object(
        config,
        &subject.org_id,
        &record.content_hash,
        &record.object_key,
    )?;
    Ok(FileContentGetResponse {
        node_id: record.node_id,
        parent_node_id: record.parent_node_id,
        name: record.name,
        version_id: record.version_id,
        content_hash: record.content_hash,
        size_bytes: record.size_bytes,
        storage_provider: record.storage_provider,
        object_key: record.object_key,
        content_hex: hex_lower(&content),
    })
}

fn content_object_put_payload(
    headers: &[(String, String)],
    body: &[u8],
    config: &RuntimeConfig,
) -> Result<ContentObjectPutResponse, (u16, ApiError)> {
    let bearer = bearer_token(headers).ok_or_else(|| {
        (
            401,
            ApiError::new("auth_required", "Authorization: Bearer token is required"),
        )
    })?;
    let subject = authenticate_subject_from_config(config, bearer, "object requests")?;
    if !scopes_allow_object_write(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot write content objects",
            ),
        ));
    }

    store_content_object(config, &subject.org_id, body)
}

fn preflight_file_content_write(
    client: &mut Client,
    subject: &AuthenticatedSubject,
    query: &FileContentPutQuery,
) -> Result<(), (u16, ApiError)> {
    if let Some(parent_node_id) = query.parent_node_id.as_deref() {
        let parent_rows = client
            .query(
                "SELECT kind FROM nodes WHERE org_id = $1 AND node_id = $2 AND deleted_at IS NULL",
                &[&subject.org_id, &parent_node_id],
            )
            .map_err(|_| {
                (
                    503,
                    ApiError::new("file_store_unavailable", "could not verify parent node"),
                )
            })?;
        let Some(parent) = parent_rows.first() else {
            return Err((
                404,
                ApiError::new("parent_not_found", "parent directory was not found"),
            ));
        };
        let kind: String = parent.get("kind");
        if kind != "directory" {
            return Err((
                409,
                ApiError::new("parent_not_directory", "parent node is not a directory"),
            ));
        }
    }

    let existing_rows = client
        .query(
            "SELECT kind FROM nodes
             WHERE org_id = $1
               AND deleted_at IS NULL
               AND (($2::text IS NULL AND parent_node_id IS NULL) OR parent_node_id = $2)
               AND lower(name) = lower($3)
             LIMIT 1",
            &[
                &subject.org_id,
                &query.parent_node_id.as_deref(),
                &query.name,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("file_store_unavailable", "could not inspect file node"),
            )
        })?;
    if let Some(existing) = existing_rows.first() {
        let kind: String = existing.get("kind");
        if kind != "file" {
            return Err((
                409,
                ApiError::new("node_kind_conflict", "existing node is not a file"),
            ));
        }
    }
    Ok(())
}

fn record_file_content(
    client: &mut Client,
    subject: &AuthenticatedSubject,
    query: FileContentPutQuery,
    stored: &ContentObjectPutResponse,
) -> Result<FileRecord, (u16, ApiError)> {
    let mut transaction = client.transaction().map_err(|_| {
        (
            503,
            ApiError::new(
                "file_store_unavailable",
                "could not start file metadata update",
            ),
        )
    })?;

    if let Some(parent_node_id) = query.parent_node_id.as_deref() {
        let parent_rows = transaction
            .query(
                "SELECT kind FROM nodes WHERE org_id = $1 AND node_id = $2 AND deleted_at IS NULL",
                &[&subject.org_id, &parent_node_id],
            )
            .map_err(|_| {
                (
                    503,
                    ApiError::new("file_store_unavailable", "could not verify parent node"),
                )
            })?;
        let Some(parent) = parent_rows.first() else {
            return Err((
                404,
                ApiError::new("parent_not_found", "parent directory was not found"),
            ));
        };
        let kind: String = parent.get("kind");
        if kind != "directory" {
            return Err((
                409,
                ApiError::new("parent_not_directory", "parent node is not a directory"),
            ));
        }
    }

    let existing_rows = transaction
        .query(
            "SELECT node_id, kind FROM nodes
             WHERE org_id = $1
               AND deleted_at IS NULL
               AND (($2::text IS NULL AND parent_node_id IS NULL) OR parent_node_id = $2)
               AND lower(name) = lower($3)
             LIMIT 1",
            &[
                &subject.org_id,
                &query.parent_node_id.as_deref(),
                &query.name,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("file_store_unavailable", "could not inspect file node"),
            )
        })?;
    let node_id = if let Some(existing) = existing_rows.first() {
        let kind: String = existing.get("kind");
        if kind != "file" {
            return Err((
                409,
                ApiError::new("node_kind_conflict", "existing node is not a file"),
            ));
        }
        existing.get("node_id")
    } else {
        let node_id = stable_node_id(
            &subject.org_id,
            query.parent_node_id.as_deref(),
            &query.name,
        );
        transaction
            .execute(
                "INSERT INTO nodes (org_id, node_id, parent_node_id, name, kind, owner_user_id, created_by, updated_by)
                 VALUES ($1, $2, $3, $4, 'file', $5, $5, $5)",
                &[
                    &subject.org_id,
                    &node_id,
                    &query.parent_node_id.as_deref(),
                    &query.name,
                    &subject.user_id,
                ],
            )
            .map_err(|_| {
                (
                    503,
                    ApiError::new("file_store_unavailable", "could not create file node"),
                )
            })?;
        node_id
    };

    let content_manifest_id = format!("cm_{}", stored.content_hash);
    let size_bytes_i64 = i64::try_from(stored.size_bytes).map_err(|_| {
        (
            400,
            ApiError::new("content_too_large", "content size exceeds database limits"),
        )
    })?;
    transaction
        .execute(
            "INSERT INTO content_manifests
               (org_id, content_manifest_id, content_hash, size_bytes, storage_provider, object_key, created_by)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (org_id, content_manifest_id) DO NOTHING",
            &[
                &subject.org_id,
                &content_manifest_id,
                &stored.content_hash,
                &size_bytes_i64,
                &stored.storage_provider,
                &stored.object_key,
                &subject.user_id,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("file_store_unavailable", "could not record content manifest"),
            )
        })?;

    let version_id = generated_id("ver");
    let operation_id: Option<String> = None;
    let parent_version_id: Option<String> = transaction
        .query_opt(
            "SELECT current_version_id FROM nodes WHERE org_id = $1 AND node_id = $2",
            &[&subject.org_id, &node_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new(
                    "file_store_unavailable",
                    "could not inspect current file version",
                ),
            )
        })?
        .and_then(|row| row.get("current_version_id"));
    transaction
        .execute(
            "INSERT INTO file_versions
               (org_id, version_id, node_id, parent_version_id, content_manifest_id, content_hash, size_bytes, created_by, source, operation_id, metadata_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, '{}'::jsonb)",
            &[
                &subject.org_id,
                &version_id,
                &node_id,
                &parent_version_id,
                &content_manifest_id,
                &stored.content_hash,
                &size_bytes_i64,
                &subject.user_id,
                &query.source,
                &operation_id,
            ],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("file_store_unavailable", "could not record file version"),
            )
        })?;
    transaction
        .execute(
            "UPDATE nodes
             SET current_version_id = $3, updated_at = now(), updated_by = $4
             WHERE org_id = $1 AND node_id = $2",
            &[&subject.org_id, &node_id, &version_id, &subject.user_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new(
                    "file_store_unavailable",
                    "could not update current file version",
                ),
            )
        })?;
    transaction.commit().map_err(|_| {
        (
            503,
            ApiError::new(
                "file_store_unavailable",
                "could not commit file metadata update",
            ),
        )
    })?;

    Ok(FileRecord {
        node_id,
        parent_node_id: query.parent_node_id,
        name: query.name,
        version_id,
        content_hash: stored.content_hash.clone(),
        size_bytes: stored.size_bytes,
        storage_provider: stored.storage_provider.clone(),
        object_key: stored.object_key.clone(),
    })
}

fn load_file_record(
    client: &mut Client,
    subject: &AuthenticatedSubject,
    node_id: &str,
) -> Result<FileRecord, (u16, ApiError)> {
    let rows = client
        .query(
            "SELECT n.node_id, n.parent_node_id, n.name, fv.version_id, fv.content_hash, fv.size_bytes, cm.storage_provider, cm.object_key
             FROM nodes n
             JOIN file_versions fv ON fv.org_id = n.org_id AND fv.version_id = n.current_version_id
             JOIN content_manifests cm ON cm.org_id = fv.org_id AND cm.content_manifest_id = fv.content_manifest_id
             WHERE n.org_id = $1 AND n.node_id = $2 AND n.kind = 'file' AND n.deleted_at IS NULL
             LIMIT 1",
            &[&subject.org_id, &node_id],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("file_store_unavailable", "could not read file metadata"),
            )
        })?;
    let Some(row) = rows.first() else {
        return Err((
            404,
            ApiError::new("file_not_found", "file node was not found"),
        ));
    };
    let size_bytes: i64 = row.get("size_bytes");
    Ok(FileRecord {
        node_id: row.get("node_id"),
        parent_node_id: row.get("parent_node_id"),
        name: row.get("name"),
        version_id: row.get("version_id"),
        content_hash: row.get("content_hash"),
        size_bytes: size_bytes as u64,
        storage_provider: row.get("storage_provider"),
        object_key: row.get("object_key"),
    })
}

fn fetch_content_object(
    config: &RuntimeConfig,
    org_id: &str,
    content_hash: &str,
    object_key: &str,
) -> Result<Vec<u8>, (u16, ApiError)> {
    object_store_check_payload_with_config(config)
        .map_err(|error| object_store_api_error(error, "object-store bucket is unavailable"))?;
    validate_sha256_hex(content_hash).map_err(|error| (400, error))?;
    let deterministic_key = content_object_key(org_id, content_hash);
    if object_key != deterministic_key {
        return Err((
            502,
            ApiError::new(
                "content_object_key_mismatch",
                "file metadata does not match object key",
            ),
        ));
    }
    let request = ObjectStoreRequest::from_config(config, "GET")
        .and_then(|request| {
            request.with_object_payload(object_key.to_string(), EMPTY_PAYLOAD_SHA256.to_string(), 0)
        })
        .map_err(|error| {
            object_store_api_error(error, "object-store get request could not be built")
        })?;
    let (response, body) = send_signed_object_store_request_for_body(&request)
        .map_err(|error| object_store_api_error(error, "object-store get request failed"))?;
    if response.status == 404 {
        return Err((
            404,
            ApiError::new("content_object_not_found", "content object was not found"),
        ));
    }
    if response.status == 401 || response.status == 403 {
        return Err((
            403,
            ApiError::new(
                "object_store_auth_failed",
                "object-store rejected configured credentials",
            ),
        ));
    }
    if response.status != 200 {
        return Err((
            503,
            ApiError::new(
                "object_store_get_failed",
                "object-store could not read content object",
            ),
        ));
    }
    if sha256_hex(&body) != content_hash {
        return Err((
            502,
            ApiError::new(
                "content_hash_mismatch",
                "downloaded content did not match file metadata hash",
            ),
        ));
    }
    Ok(body)
}

fn stable_node_id(org_id: &str, parent_node_id: Option<&str>, name: &str) -> String {
    let seed = format!(
        "{org_id}\n{}\n{}",
        parent_node_id.unwrap_or(""),
        name.to_ascii_lowercase()
    );
    format!("node_{}", &sha256_hex(seed.as_bytes())[..32])
}

fn generated_id(prefix: &str) -> String {
    let now = OffsetDateTime::now_utc().unix_timestamp_nanos();
    let seed = format!("{prefix}:{now}:{}", std::process::id());
    format!("{prefix}_{}", &sha256_hex(seed.as_bytes())[..32])
}

fn store_content_object(
    config: &RuntimeConfig,
    org_id: &str,
    body: &[u8],
) -> Result<ContentObjectPutResponse, (u16, ApiError)> {
    object_store_check_payload_with_config(config)
        .map_err(|error| object_store_api_error(error, "object-store bucket is unavailable"))?;

    let content_hash = sha256_hex(body);
    let object_key = content_object_key(org_id, &content_hash);
    let request = ObjectStoreRequest::from_config(config, "PUT")
        .and_then(|request| {
            request.with_object_payload(object_key.clone(), content_hash.clone(), body.len())
        })
        .map_err(|error| {
            object_store_api_error(error, "object-store put request could not be built")
        })?;
    let response = send_signed_object_store_request_with_payload(&request, body)
        .map_err(|error| object_store_api_error(error, "object-store put request failed"))?;
    if response.status != 200 {
        return Err((
            if response.status == 401 || response.status == 403 {
                403
            } else {
                503
            },
            ApiError::new(
                "object_store_put_failed",
                "object-store did not accept content object",
            ),
        ));
    }

    Ok(ContentObjectPutResponse {
        content_hash,
        size_bytes: body.len() as u64,
        storage_provider: config.object_store.provider.clone(),
        object_key,
    })
}

fn content_object_get_payload(
    query: &str,
    headers: &[(String, String)],
    config: &RuntimeConfig,
) -> Result<ContentObjectGetResponse, (u16, ApiError)> {
    let content_hash = parse_content_hash_query(query)?;
    let bearer = bearer_token(headers).ok_or_else(|| {
        (
            401,
            ApiError::new("auth_required", "Authorization: Bearer token is required"),
        )
    })?;
    let subject = authenticate_subject_from_config(config, bearer, "object requests")?;
    if !scopes_allow_object_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot read content objects",
            ),
        ));
    }

    object_store_check_payload_with_config(config)
        .map_err(|error| object_store_api_error(error, "object-store bucket is unavailable"))?;

    let object_key = content_object_key(&subject.org_id, &content_hash);
    let request = ObjectStoreRequest::from_config(config, "GET")
        .and_then(|request| {
            request.with_object_payload(object_key.clone(), EMPTY_PAYLOAD_SHA256.to_string(), 0)
        })
        .map_err(|error| {
            object_store_api_error(error, "object-store get request could not be built")
        })?;
    let (response, body) = send_signed_object_store_request_for_body(&request)
        .map_err(|error| object_store_api_error(error, "object-store get request failed"))?;
    if response.status == 404 {
        return Err((
            404,
            ApiError::new("content_object_not_found", "content object was not found"),
        ));
    }
    if response.status == 401 || response.status == 403 {
        return Err((
            403,
            ApiError::new(
                "object_store_auth_failed",
                "object-store rejected configured credentials",
            ),
        ));
    }
    if response.status != 200 {
        return Err((
            503,
            ApiError::new(
                "object_store_get_failed",
                "object-store could not read content object",
            ),
        ));
    }
    let actual_hash = sha256_hex(&body);
    if actual_hash != content_hash {
        return Err((
            502,
            ApiError::new(
                "content_hash_mismatch",
                "downloaded content did not match requested hash",
            ),
        ));
    }

    Ok(ContentObjectGetResponse {
        content_hash,
        size_bytes: body.len() as u64,
        storage_provider: config.object_store.provider.clone(),
        object_key,
        content_hex: hex_lower(&body),
    })
}

fn authenticate_subject_from_config(
    config: &RuntimeConfig,
    bearer: &str,
    purpose: &str,
) -> Result<AuthenticatedSubject, (u16, ApiError)> {
    let database_url = database_url_from_config(config).map_err(|error| {
        (
            503,
            ApiError::new(
                error.code(),
                format!("database is not configured for {purpose}"),
            ),
        )
    })?;
    let mut client = connect_database(database_url).map_err(|error| {
        (
            503,
            ApiError::new(
                error.code(),
                format!("database is unavailable for {purpose}"),
            ),
        )
    })?;
    authenticate_subject(&mut client, bearer)
}

fn object_store_api_error(
    error: ObjectStoreError,
    fallback_message: &'static str,
) -> (u16, ApiError) {
    let status = match error.code() {
        "object_store_endpoint_missing"
        | "object_store_endpoint_invalid"
        | "object_store_endpoint_unsupported"
        | "object_store_endpoint_insecure"
        | "object_store_bucket_missing_config"
        | "object_store_bucket_invalid"
        | "object_store_access_key_missing"
        | "object_store_access_key_invalid"
        | "object_store_secret_missing"
        | "object_store_region_invalid"
        | "object_store_object_key_invalid"
        | "object_store_payload_hash_invalid" => 400,
        "object_store_auth_failed" => 403,
        _ => 503,
    };
    (status, ApiError::new(error.code(), fallback_message))
}

fn content_object_key(org_id: &str, content_hash: &str) -> String {
    format!("orgs/{org_id}/content/sha256/{content_hash}")
}

fn parse_content_hash_query(query: &str) -> Result<String, (u16, ApiError)> {
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if key == "sha256" || key == "content_hash" {
            let value = value.trim().to_ascii_lowercase();
            validate_sha256_hex(&value).map_err(|error| (400, error))?;
            return Ok(value);
        }
    }
    Err((
        400,
        ApiError::new(
            "content_hash_required",
            "sha256 query parameter is required",
        ),
    ))
}

fn parse_file_content_put_query(query: &str) -> Result<FileContentPutQuery, (u16, ApiError)> {
    let mut parent_node_id = None;
    let mut name = None;
    let mut source = "cli".to_string();

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "parent" | "parent_node_id" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_node_id_value(&decoded)?;
                parent_node_id = Some(decoded);
            }
            "name" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_file_name(&decoded)?;
                name = Some(decoded);
            }
            "source" if !value.trim().is_empty() => {
                let decoded = percent_decode_query_value(value)?;
                validate_file_source(&decoded)?;
                source = decoded;
            }
            "parent" | "parent_node_id" | "name" | "source" => {}
            _ => {}
        }
    }

    let name = name.ok_or_else(|| {
        (
            400,
            ApiError::new("file_name_required", "name query parameter is required"),
        )
    })?;
    Ok(FileContentPutQuery {
        parent_node_id,
        name,
        source,
    })
}

fn parse_file_content_get_query(query: &str) -> Result<FileContentGetQuery, (u16, ApiError)> {
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if (key == "node" || key == "node_id") && !value.trim().is_empty() {
            let node_id = percent_decode_query_value(value)?;
            validate_node_id_value(&node_id)?;
            return Ok(FileContentGetQuery { node_id });
        }
    }
    Err((
        400,
        ApiError::new("node_id_required", "node_id query parameter is required"),
    ))
}

fn percent_decode_query_value(value: &str) -> Result<String, (u16, ApiError)> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return Err((
                        400,
                        ApiError::new(
                            "invalid_query_encoding",
                            "query parameter is not valid percent encoding",
                        ),
                    ));
                }
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).map_err(|_| {
                    (
                        400,
                        ApiError::new(
                            "invalid_query_encoding",
                            "query parameter is not valid percent encoding",
                        ),
                    )
                })?;
                let byte = u8::from_str_radix(hex, 16).map_err(|_| {
                    (
                        400,
                        ApiError::new(
                            "invalid_query_encoding",
                            "query parameter is not valid percent encoding",
                        ),
                    )
                })?;
                output.push(byte);
                index += 3;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).map_err(|_| {
        (
            400,
            ApiError::new(
                "invalid_query_encoding",
                "query parameter is not valid UTF-8",
            ),
        )
    })
}

fn validate_file_name(name: &str) -> Result<(), (u16, ApiError)> {
    let is_valid = !name.trim().is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && !name.bytes().any(|byte| byte.is_ascii_control());
    if is_valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new(
                "invalid_file_name",
                "file name is not valid for the MVP file API",
            ),
        ))
    }
}

fn validate_node_id_value(node_id: &str) -> Result<(), (u16, ApiError)> {
    let is_valid = !node_id.trim().is_empty()
        && node_id.len() <= 128
        && node_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'));
    if is_valid {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new(
                "invalid_node_id",
                "node_id is not valid for the MVP file API",
            ),
        ))
    }
}

fn validate_file_source(source: &str) -> Result<(), (u16, ApiError)> {
    if matches!(source, "ui" | "cli" | "agent" | "api" | "server" | "test") {
        Ok(())
    } else {
        Err((
            400,
            ApiError::new("invalid_source", "source is not valid for file operations"),
        ))
    }
}

fn parse_namespace_children_query(query: &str) -> Result<NamespaceChildrenQuery, (u16, ApiError)> {
    let mut parent_node_id = None;
    let mut limit = DEFAULT_NAMESPACE_LIMIT;

    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "parent" | "parent_node_id" if !value.trim().is_empty() => {
                parent_node_id = Some(value.trim().to_string());
            }
            "limit" if !value.trim().is_empty() => {
                let parsed = value.trim().parse::<u32>().map_err(|_| {
                    (
                        400,
                        ApiError::new("invalid_limit", "limit must be a positive integer"),
                    )
                })?;
                if parsed == 0 || parsed > MAX_NAMESPACE_LIMIT {
                    return Err((
                        400,
                        ApiError::new(
                            "invalid_limit",
                            format!("limit must be between 1 and {MAX_NAMESPACE_LIMIT}"),
                        ),
                    ));
                }
                limit = parsed;
            }
            "parent" | "parent_node_id" | "limit" => {}
            _ => {}
        }
    }

    Ok(NamespaceChildrenQuery {
        parent_node_id,
        limit,
    })
}

fn bearer_token(headers: &[(String, String)]) -> Option<&str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        .and_then(|(_, value)| {
            let value = value.trim();
            value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("bearer "))
                .map(str::trim)
                .filter(|token| !token.is_empty())
        })
}

fn authenticate_subject(
    client: &mut Client,
    bearer: &str,
) -> Result<AuthenticatedSubject, (u16, ApiError)> {
    let secret_hash = secret_hash_for_token(bearer);
    let rows = client
        .query(
            "SELECT t.org_id, t.user_id, t.scopes::text AS scopes_json
             FROM tokens t
             JOIN users u ON u.org_id = t.org_id AND u.user_id = t.user_id
             JOIN organizations o ON o.org_id = t.org_id
             WHERE t.secret_hash = $1
               AND t.status = 'active'
               AND (t.expires_at IS NULL OR t.expires_at > now())
               AND t.revoked_at IS NULL
               AND u.status = 'active'
               AND o.status = 'active'
             LIMIT 2",
            &[&secret_hash],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new("auth_store_unavailable", "could not verify bearer token"),
            )
        })?;

    let Some(row) = rows.first() else {
        return Err((
            401,
            ApiError::new("auth_invalid", "bearer token is not valid for this server"),
        ));
    };
    if rows.len() > 1 {
        return Err((
            401,
            ApiError::new(
                "auth_ambiguous",
                "bearer token is not valid for this server",
            ),
        ));
    }

    Ok(AuthenticatedSubject {
        org_id: row.get("org_id"),
        user_id: row.get("user_id"),
        scopes_json: row.get("scopes_json"),
    })
}

fn scopes_allow_namespace_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["namespace:read", "namespace:*", "server:read"],
    )
}

fn scopes_allow_object_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &[
            "object:read",
            "object:*",
            "file:read",
            "file:*",
            "server:read",
        ],
    )
}

fn scopes_allow_object_write(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &[
            "object:write",
            "object:*",
            "file:write",
            "file:*",
            "server:write",
        ],
    )
}

fn scopes_allow_file_read(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["file:read", "file:*", "server:read"])
}

fn scopes_allow_file_write(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["file:write", "file:*", "server:write"])
}

fn scopes_allow_lock_read(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["lock:read", "lock:*", "server:read"])
}

fn scopes_allow_lock_write(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["lock:write", "lock:*", "server:write"])
}

fn scopes_allow_conflict_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &[
            "conflict:read",
            "conflict:*",
            "namespace:read",
            "server:read",
        ],
    )
}

fn scopes_allow_operation_write(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["operation:write", "operation:*", "server:write"],
    )
}

fn scopes_allow_trash_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["trash:read", "trash:*", "namespace:read", "server:read"],
    )
}

fn scopes_allow_audit_read(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["audit:read", "audit:*", "server:read"])
}

fn scopes_allow_devices_read(scopes_json: &str) -> bool {
    scopes_allow_any(scopes_json, &["device:read", "device:*", "server:read"])
}

fn scopes_allow_projects_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["project:read", "project:*", "namespace:read", "server:read"],
    )
}

fn scopes_allow_worksets_read(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["workset:read", "workset:*", "namespace:read", "server:read"],
    )
}

fn scopes_allow_any(scopes_json: &str, allowed_scopes: &[&str]) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(scopes_json) else {
        return false;
    };
    value.as_array().is_some_and(|scopes| {
        scopes
            .iter()
            .filter_map(|scope| scope.as_str())
            .any(|scope| scope == "*" || allowed_scopes.contains(&scope))
    })
}

fn list_namespace_children(
    client: &mut Client,
    subject: &AuthenticatedSubject,
    query: NamespaceChildrenQuery,
) -> Result<NamespaceChildrenResponse, (u16, ApiError)> {
    if !scopes_allow_namespace_read(&subject.scopes_json) {
        return Err((
            403,
            ApiError::new(
                "auth_scope_missing",
                "bearer token cannot read namespace metadata",
            ),
        ));
    }
    let parent = query.parent_node_id.as_deref();
    let limit = i64::from(query.limit);
    let rows = client
        .query(
            "SELECT node_id, parent_node_id, name, kind, current_version_id
             FROM nodes
             WHERE org_id = $1
               AND deleted_at IS NULL
               AND (($2::text IS NULL AND parent_node_id IS NULL) OR parent_node_id = $2)
             ORDER BY lower(name), node_id
             LIMIT $3",
            &[&subject.org_id, &parent, &limit],
        )
        .map_err(|_| {
            (
                503,
                ApiError::new(
                    "namespace_store_unavailable",
                    "could not list namespace children",
                ),
            )
        })?;

    Ok(NamespaceChildrenResponse {
        parent_node_id: query.parent_node_id,
        limit: query.limit,
        nodes: rows
            .into_iter()
            .map(|row| NamespaceNodeSummary {
                node_id: row.get("node_id"),
                parent_node_id: row.get("parent_node_id"),
                name: row.get("name"),
                kind: row.get("kind"),
                current_version_id: row.get("current_version_id"),
            })
            .collect(),
    })
}

fn secret_hash_for_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    format!("sha256:{hex}")
}

#[derive(Clone, PartialEq, Eq)]
struct ObjectStoreRequest {
    method: &'static str,
    provider: String,
    endpoint: ObjectStoreEndpoint,
    bucket: String,
    object_key: Option<String>,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    payload_sha256: String,
    content_length: usize,
}

impl ObjectStoreRequest {
    fn from_config(config: &RuntimeConfig, method: &'static str) -> Result<Self, ObjectStoreError> {
        let endpoint = config
            .object_store
            .endpoint
            .as_deref()
            .ok_or_else(|| {
                ObjectStoreError::new(
                    "object_store_endpoint_missing",
                    "object-store endpoint is not configured",
                )
            })
            .and_then(parse_object_store_endpoint)?;
        let bucket = config
            .object_store
            .bucket
            .as_deref()
            .ok_or_else(|| {
                ObjectStoreError::new(
                    "object_store_bucket_missing_config",
                    "object-store bucket is not configured",
                )
            })?
            .trim();
        validate_bucket_name(bucket)?;
        let access_key_id = config
            .object_store
            .access_key_id_for_process_boundary()
            .ok_or_else(|| {
                ObjectStoreError::new(
                    "object_store_access_key_missing",
                    "object-store access key ID is not configured",
                )
            })?
            .expose_for_process_boundary()
            .to_string();
        validate_access_key_id(&access_key_id)?;
        let secret_access_key = config
            .object_store
            .secret_access_key
            .as_ref()
            .ok_or_else(|| {
                ObjectStoreError::new(
                    "object_store_secret_missing",
                    "object-store secret access key is not configured",
                )
            })?
            .expose_for_process_boundary()
            .to_string();
        let region = config
            .object_store
            .region
            .as_deref()
            .filter(|region| !region.trim().is_empty())
            .unwrap_or(DEFAULT_OBJECT_STORE_REGION)
            .trim()
            .to_string();
        validate_region(&region)?;

        Ok(Self {
            method,
            provider: config.object_store.provider.clone(),
            endpoint,
            bucket: bucket.to_string(),
            object_key: None,
            region,
            access_key_id,
            secret_access_key,
            payload_sha256: EMPTY_PAYLOAD_SHA256.to_string(),
            content_length: 0,
        })
    }

    fn with_object_payload(
        mut self,
        object_key: String,
        payload_sha256: String,
        content_length: usize,
    ) -> Result<Self, ObjectStoreError> {
        validate_object_key(&object_key)?;
        validate_sha256_hex(&payload_sha256).map_err(|_| {
            ObjectStoreError::new(
                "object_store_payload_hash_invalid",
                "object-store payload hash is not valid",
            )
        })?;
        self.object_key = Some(object_key);
        self.payload_sha256 = payload_sha256;
        self.content_length = content_length;
        Ok(self)
    }

    fn report(&self, status: impl Into<String>, http_status: u16) -> ObjectStoreCheckReport {
        ObjectStoreCheckReport {
            name: "object_store".to_string(),
            provider: self.provider.clone(),
            endpoint_configured: true,
            bucket: self.bucket.clone(),
            region: self.region.clone(),
            credentials_configured: true,
            status: status.into(),
            http_status,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectStoreEndpoint {
    host: String,
    host_header: String,
    port: u16,
    canonical_bucket_prefix: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObjectStoreHttpResponse {
    status: u16,
}

fn parse_object_store_endpoint(endpoint: &str) -> Result<ObjectStoreEndpoint, ObjectStoreError> {
    let rest = endpoint.trim().strip_prefix("http://").ok_or_else(|| {
        ObjectStoreError::new(
            "object_store_endpoint_unsupported",
            "object-store endpoint must start with http:// until server TLS support lands",
        )
    })?;
    if rest
        .bytes()
        .any(|byte| byte.is_ascii_control() || byte == b' ')
        || rest.contains('@')
    {
        return Err(ObjectStoreError::new(
            "object_store_endpoint_invalid",
            "object-store endpoint authority or path contains invalid characters",
        ));
    }
    let (authority, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port, host_header) = parse_object_store_authority(authority)?;
    if host.trim().is_empty() {
        return Err(ObjectStoreError::new(
            "object_store_endpoint_invalid",
            "object-store endpoint host is empty",
        ));
    }
    if !is_internal_http_object_store_host(&host) {
        return Err(ObjectStoreError::new(
            "object_store_endpoint_insecure",
            "http object-store endpoints must use loopback, private IPs, or internal service names until TLS support lands",
        ));
    }
    let base_path = base_path.trim_matches('/');
    let valid_base_path = base_path.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b'~')
    });
    if !valid_base_path
        || base_path.contains('?')
        || base_path.contains('#')
        || base_path.contains("..")
    {
        return Err(ObjectStoreError::new(
            "object_store_endpoint_invalid",
            "object-store endpoint path is invalid",
        ));
    }
    Ok(ObjectStoreEndpoint {
        host,
        host_header,
        port,
        canonical_bucket_prefix: base_path.to_string(),
    })
}

fn parse_object_store_authority(
    authority: &str,
) -> Result<(String, u16, String), ObjectStoreError> {
    if let Some(without_opening_bracket) = authority.strip_prefix('[') {
        let (host, after_host) = without_opening_bracket.split_once(']').ok_or_else(|| {
            ObjectStoreError::new(
                "object_store_endpoint_invalid",
                "object-store IPv6 endpoint is missing a closing bracket",
            )
        })?;
        let port = match after_host.strip_prefix(':') {
            Some(port) => parse_object_store_port(port)?,
            None if after_host.is_empty() => 80,
            None => {
                return Err(ObjectStoreError::new(
                    "object_store_endpoint_invalid",
                    "object-store IPv6 endpoint has invalid authority syntax",
                ));
            }
        };
        return Ok((host.to_string(), port, authority.to_string()));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => Ok((
            host.to_string(),
            parse_object_store_port(port)?,
            authority.to_string(),
        )),
        None => Ok((authority.to_string(), 80, authority.to_string())),
    }
}

fn is_internal_http_object_store_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return is_internal_object_store_ip(ip);
    }
    let is_safe_dns_char = |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.');
    let is_safe_name = !host.is_empty() && host.bytes().all(is_safe_dns_char);
    is_safe_name
        && (!host.contains('.')
            || host.ends_with(".local")
            || host.ends_with(".svc")
            || host.ends_with(".svc.cluster.local"))
}

fn is_internal_object_store_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => ip.is_loopback() || ip.is_private(),
        std::net::IpAddr::V6(ip) => ip.is_loopback() || ip.is_unique_local(),
    }
}

fn parse_object_store_port(port: &str) -> Result<u16, ObjectStoreError> {
    port.parse::<u16>().map_err(|_| {
        ObjectStoreError::new(
            "object_store_endpoint_invalid",
            "object-store endpoint port is not valid",
        )
    })
}

fn validate_access_key_id(access_key_id: &str) -> Result<(), ObjectStoreError> {
    let is_valid = !access_key_id.trim().is_empty()
        && access_key_id.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'-' | b'+' | b'=' | b',' | b'@')
        });
    if is_valid {
        Ok(())
    } else {
        Err(ObjectStoreError::new(
            "object_store_access_key_invalid",
            "object-store access key ID contains invalid characters",
        ))
    }
}

fn validate_region(region: &str) -> Result<(), ObjectStoreError> {
    let is_valid = !region.trim().is_empty()
        && region
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    if is_valid {
        Ok(())
    } else {
        Err(ObjectStoreError::new(
            "object_store_region_invalid",
            "object-store region contains invalid characters",
        ))
    }
}

fn validate_sha256_hex(value: &str) -> Result<(), ApiError> {
    let is_valid = value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if is_valid {
        Ok(())
    } else {
        Err(ApiError::new(
            "invalid_content_hash",
            "content hash must be a 64-character SHA-256 hex digest",
        ))
    }
}

fn validate_object_key(object_key: &str) -> Result<(), ObjectStoreError> {
    let valid_length = !object_key.is_empty() && object_key.len() <= 512;
    let valid_chars = object_key.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'-' | b'_' | b'~' | b':')
    });
    let valid_shape = !object_key.starts_with('/')
        && !object_key.ends_with('/')
        && !object_key.contains("//")
        && !object_key.contains("..")
        && !object_key.contains('?')
        && !object_key.contains('#');
    if valid_length && valid_chars && valid_shape {
        Ok(())
    } else {
        Err(ObjectStoreError::new(
            "object_store_object_key_invalid",
            "object-store object key is not valid",
        ))
    }
}

fn validate_bucket_name(bucket: &str) -> Result<(), ObjectStoreError> {
    let valid_length = (3..=63).contains(&bucket.len());
    let valid_chars = bucket.bytes().all(|byte| {
        byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
    });
    let valid_edges = !bucket.starts_with(['.', '-']) && !bucket.ends_with(['.', '-']);
    let valid_sequences =
        !bucket.contains("..") && !bucket.contains(".-") && !bucket.contains("-.");
    if valid_length && valid_chars && valid_edges && valid_sequences {
        Ok(())
    } else {
        Err(ObjectStoreError::new(
            "object_store_bucket_invalid",
            "object-store bucket name is not valid",
        ))
    }
}

fn send_signed_object_store_request(
    request: &ObjectStoreRequest,
) -> Result<ObjectStoreHttpResponse, ObjectStoreError> {
    send_signed_object_store_request_with_payload(request, &[])
}

fn send_signed_object_store_request_with_payload(
    request: &ObjectStoreRequest,
    payload: &[u8],
) -> Result<ObjectStoreHttpResponse, ObjectStoreError> {
    let mut reader = send_signed_object_store_request_reader(request, payload)?;
    let status_line = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES).map_err(|_| {
        ObjectStoreError::new("object_store_unavailable", "object-store read failed")
    })?;
    let status = http_status_from_line(&status_line)?;
    Ok(ObjectStoreHttpResponse { status })
}

fn send_signed_object_store_request_for_body(
    request: &ObjectStoreRequest,
) -> Result<(ObjectStoreHttpResponse, Vec<u8>), ObjectStoreError> {
    let mut reader = send_signed_object_store_request_reader(request, &[])?;
    let status_line = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES).map_err(|_| {
        ObjectStoreError::new("object_store_unavailable", "object-store read failed")
    })?;
    let status = http_status_from_line(&status_line)?;
    let mut content_length = None;
    for _ in 0..MAX_HEADERS {
        let header = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES).map_err(|_| {
            ObjectStoreError::new("object_store_unavailable", "object-store read failed")
        })?;
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse::<usize>().ok();
        }
    }
    let mut body = Vec::new();
    match content_length {
        Some(length) if length <= MAX_OBJECT_RESPONSE_BYTES => {
            body.resize(length, 0);
            reader.read_exact(&mut body).map_err(|_| {
                ObjectStoreError::new("object_store_unavailable", "object-store read failed")
            })?;
        }
        Some(_) => {
            return Err(ObjectStoreError::new(
                "object_store_response_too_large",
                "object-store response exceeded the MVP download limit",
            ));
        }
        None => {
            reader
                .take(MAX_OBJECT_RESPONSE_BYTES as u64 + 1)
                .read_to_end(&mut body)
                .map_err(|_| {
                    ObjectStoreError::new("object_store_unavailable", "object-store read failed")
                })?;
            if body.len() > MAX_OBJECT_RESPONSE_BYTES {
                return Err(ObjectStoreError::new(
                    "object_store_response_too_large",
                    "object-store response exceeded the MVP download limit",
                ));
            }
        }
    }
    Ok((ObjectStoreHttpResponse { status }, body))
}

fn send_signed_object_store_request_reader(
    request: &ObjectStoreRequest,
    payload: &[u8],
) -> Result<BufReader<TcpStream>, ObjectStoreError> {
    if request.content_length != payload.len() {
        return Err(ObjectStoreError::new(
            "object_store_payload_length_mismatch",
            "object-store payload length did not match signed length",
        ));
    }
    let now = OffsetDateTime::now_utc();
    let amz_date = now.format(AWS_DATETIME_FORMAT).map_err(|_| {
        ObjectStoreError::new(
            "object_store_signing_failed",
            "object-store request signing failed",
        )
    })?;
    let date = now.format(AWS_DATE_FORMAT).map_err(|_| {
        ObjectStoreError::new(
            "object_store_signing_failed",
            "object-store request signing failed",
        )
    })?;
    let canonical_uri = object_store_request_uri(request);
    let credential_scope = format!("{date}/{}/{}/{}", request.region, S3_SERVICE, AWS4_REQUEST);
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{}\n{}\n\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n\n{}\n{}",
        request.method,
        canonical_uri,
        request.endpoint.host_header,
        request.payload_sha256,
        amz_date,
        signed_headers,
        request.payload_sha256
    );
    let canonical_request_hash = sha256_hex(canonical_request.as_bytes());
    let string_to_sign =
        format!("AWS4-HMAC-SHA256\n{amz_date}\n{credential_scope}\n{canonical_request_hash}");
    let signing_key = aws_v4_signing_key(&request.secret_access_key, &date, &request.region)?;
    let signature = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes())?;
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
        request.access_key_id, credential_scope, signed_headers, signature
    );

    let addresses = (request.endpoint.host.as_str(), request.endpoint.port)
        .to_socket_addrs()
        .map_err(|_| {
            ObjectStoreError::new(
                "object_store_unavailable",
                "object-store endpoint could not be resolved",
            )
        })?;
    let mut saw_address = false;
    let mut saw_internal_address = false;
    let mut stream = None;
    for address in addresses {
        saw_address = true;
        if !is_internal_object_store_ip(address.ip()) {
            continue;
        }
        saw_internal_address = true;
        if let Ok(connected) = TcpStream::connect_timeout(&address, Duration::from_secs(5)) {
            stream = Some(connected);
            break;
        }
    }
    let mut stream = stream.ok_or_else(|| {
        if saw_address && !saw_internal_address {
            ObjectStoreError::new(
                "object_store_endpoint_insecure",
                "object-store endpoint did not resolve to a loopback or private address",
            )
        } else {
            ObjectStoreError::new(
                "object_store_unavailable",
                "object-store endpoint could not be reached",
            )
        }
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| {
            ObjectStoreError::new("object_store_unavailable", "object-store read failed")
        })?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| {
            ObjectStoreError::new("object_store_unavailable", "object-store write failed")
        })?;

    write!(
        stream,
        "{} {} HTTP/1.1\r\nHost: {}\r\nAuthorization: {}\r\nx-amz-content-sha256: {}\r\nx-amz-date: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        request.method,
        canonical_uri,
        request.endpoint.host_header,
        authorization,
        request.payload_sha256,
        amz_date,
        request.content_length
    )
    .map_err(|_| ObjectStoreError::new("object_store_unavailable", "object-store write failed"))?;
    if !payload.is_empty() {
        stream.write_all(payload).map_err(|_| {
            ObjectStoreError::new("object_store_unavailable", "object-store write failed")
        })?;
    }
    stream.flush().map_err(|_| {
        ObjectStoreError::new("object_store_unavailable", "object-store write failed")
    })?;

    Ok(BufReader::new(stream))
}

fn http_status_from_line(status_line: &str) -> Result<u16, ObjectStoreError> {
    status_line
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| {
            ObjectStoreError::new(
                "object_store_protocol_error",
                "object-store response did not include a valid HTTP status",
            )
        })
}

fn object_store_request_uri(request: &ObjectStoreRequest) -> String {
    let bucket_uri = object_store_bucket_uri(&request.endpoint, &request.bucket);
    match request.object_key.as_deref() {
        Some(object_key) => format!("{bucket_uri}/{object_key}"),
        None => bucket_uri,
    }
}

fn object_store_bucket_uri(endpoint: &ObjectStoreEndpoint, bucket: &str) -> String {
    if endpoint.canonical_bucket_prefix.is_empty() {
        format!("/{bucket}")
    } else {
        format!("/{}/{bucket}", endpoint.canonical_bucket_prefix)
    }
}

fn aws_v4_signing_key(
    secret_access_key: &str,
    date: &str,
    region: &str,
) -> Result<Vec<u8>, ObjectStoreError> {
    let date_key = hmac_sha256_bytes(
        format!("AWS4{secret_access_key}").as_bytes(),
        date.as_bytes(),
    )?;
    let region_key = hmac_sha256_bytes(&date_key, region.as_bytes())?;
    let service_key = hmac_sha256_bytes(&region_key, S3_SERVICE.as_bytes())?;
    hmac_sha256_bytes(&service_key, AWS4_REQUEST.as_bytes())
}

fn hmac_sha256_bytes(key: &[u8], payload: &[u8]) -> Result<Vec<u8>, ObjectStoreError> {
    let mut mac = HmacSha256::new_from_slice(key).map_err(|_| {
        ObjectStoreError::new(
            "object_store_signing_failed",
            "object-store request signing failed",
        )
    })?;
    mac.update(payload);
    Ok(mac.finalize().into_bytes().to_vec())
}

fn hmac_sha256_hex(key: &[u8], payload: &[u8]) -> Result<String, ObjectStoreError> {
    hmac_sha256_bytes(key, payload).map(|bytes| hex_lower(&bytes))
}

fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn split_path_and_query(path: &str) -> (&str, &str) {
    path.split_once('?').unwrap_or((path, ""))
}

pub fn run_migrations_from_env() -> Result<MigrationReport, MigrationError> {
    let config = RuntimeConfig::from_env();
    run_migrations_from_config(&config)
}

pub fn run_migrations_from_config(
    config: &RuntimeConfig,
) -> Result<MigrationReport, MigrationError> {
    let database_url = database_url_from_config(config)?;
    run_migrations(database_url)
}

pub fn migrate_with_database_url(
    database_url: Option<&str>,
) -> Result<MigrationReport, MigrationError> {
    let database_url = validate_database_url(database_url)?;
    run_migrations(database_url)
}

fn run_migrations(database_url: &str) -> Result<MigrationReport, MigrationError> {
    let mut client = connect_database(database_url)?;

    acquire_migration_lock(&mut client)?;

    client
        .batch_execute(SCHEMA_MIGRATIONS_TABLE_SQL)
        .map_err(|_| {
            MigrationError::new(
                "migration_store_unavailable",
                "Could not verify or update server migration state",
            )
        })?;

    let applied_migrations_by_version = applied_migrations_by_version(&mut client)?;
    verify_no_unknown_applied_migrations(&applied_migrations_by_version)?;

    let mut applied_migrations = Vec::new();
    let mut already_applied_migrations = Vec::new();

    for migration in migrations() {
        let summary = migration.summary();
        if let Some(applied) = applied_migrations_by_version.get(migration.version) {
            verify_applied_migration_matches(&summary, applied)?;
            already_applied_migrations.push(summary);
            continue;
        }

        apply_migration(&mut client, migration, &summary)?;
        applied_migrations.push(summary);
    }

    let status = if applied_migrations.is_empty() {
        "up_to_date"
    } else {
        "applied"
    };

    Ok(MigrationReport {
        name: "biohazardfs-server".to_string(),
        mode: "migrate".to_string(),
        status: status.to_string(),
        database_configured: true,
        migration_count: migrations().len(),
        current_version: migrations()
            .last()
            .map(|migration| migration.version.to_string()),
        applied_migrations,
        already_applied_migrations,
    })
}

fn acquire_migration_lock(client: &mut Client) -> Result<(), MigrationError> {
    client
        .execute(
            "SELECT pg_advisory_lock($1)",
            &[&ADVISORY_MIGRATION_LOCK_ID],
        )
        .map(|_| ())
        .map_err(|_| {
            MigrationError::new(
                "migration_lock_unavailable",
                "Could not acquire server database migration lock",
            )
        })
}

fn applied_migrations_by_version(
    client: &mut Client,
) -> Result<std::collections::BTreeMap<String, AppliedMigration>, MigrationError> {
    let rows = client
        .query(
            "SELECT version, name, checksum FROM schema_migrations ORDER BY version",
            &[],
        )
        .map_err(|_| {
            MigrationError::new(
                "migration_store_unavailable",
                "Could not verify or update server migration state",
            )
        })?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let applied = AppliedMigration {
                version: row.get::<_, String>("version"),
                name: row.get::<_, String>("name"),
                checksum: row.get::<_, String>("checksum"),
            };
            (applied.version.clone(), applied)
        })
        .collect())
}

fn verify_no_unknown_applied_migrations(
    applied_migrations_by_version: &std::collections::BTreeMap<String, AppliedMigration>,
) -> Result<(), MigrationError> {
    let bundled_versions = migrations()
        .iter()
        .map(|migration| migration.version)
        .collect::<std::collections::BTreeSet<_>>();

    if let Some(unknown_version) = applied_migrations_by_version
        .keys()
        .find(|version| !bundled_versions.contains(version.as_str()))
    {
        return Err(MigrationError::with_details(
            "migration_version_unsupported",
            "Database has a server migration version that is newer than this BiohazardFS binary supports",
            serde_json::json!({
                "recorded_migration_version": unknown_version,
            }),
        ));
    }

    Ok(())
}

fn verify_applied_migration_matches(
    expected: &MigrationSummary,
    applied: &AppliedMigration,
) -> Result<(), MigrationError> {
    if applied.name != expected.name || applied.checksum != expected.checksum {
        return Err(MigrationError::with_details(
            "migration_checksum_mismatch",
            "Recorded server database migration does not match the bundled migration",
            serde_json::json!({
                "migration_version": expected.version,
                "expected_name": expected.name,
                "recorded_name": applied.name,
            }),
        ));
    }
    Ok(())
}

fn apply_migration(
    client: &mut Client,
    migration: &Migration,
    summary: &MigrationSummary,
) -> Result<(), MigrationError> {
    let mut transaction = client.transaction().map_err(|_| {
        migration_error_with_version(migration, "Could not start database migration transaction")
    })?;

    transaction.batch_execute(migration.sql).map_err(|_| {
        migration_error_with_version(migration, "Could not apply database migration")
    })?;

    transaction
        .execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES ($1, $2, $3, now())",
            &[&summary.version, &summary.name, &summary.checksum],
        )
        .map_err(|_| migration_error_with_version(migration, "Could not record database migration"))?;

    transaction.commit().map_err(|_| {
        migration_error_with_version(migration, "Could not commit database migration")
    })?;

    Ok(())
}

fn verify_latest_migration_from_config(config: &RuntimeConfig) -> Result<(), MigrationError> {
    let database_url = database_url_from_config(config)?;
    verify_latest_migration(database_url)
}

fn verify_latest_migration(database_url: &str) -> Result<(), MigrationError> {
    let mut client = connect_database(database_url)?;

    let applied_migrations_by_version = applied_migrations_by_version(&mut client)?;
    verify_no_unknown_applied_migrations(&applied_migrations_by_version)?;
    for migration in migrations() {
        let summary = migration.summary();
        let Some(applied) = applied_migrations_by_version.get(migration.version) else {
            return Err(MigrationError::new(
                "migration_not_verified",
                "Could not verify server database migrations",
            ));
        };
        verify_applied_migration_matches(&summary, applied)?;
    }

    Ok(())
}

fn database_url_from_config(config: &RuntimeConfig) -> Result<&str, MigrationError> {
    let database_url = config
        .database
        .url_for_process_boundary()
        .map(|url| url.expose_for_process_boundary());
    validate_database_url(database_url)
}

fn connect_database(database_url: &str) -> Result<Client, MigrationError> {
    let mut config = database_url.parse::<Config>().map_err(|_| {
        MigrationError::new(
            "database_url_invalid",
            "BIOHAZARDFS_DATABASE_URL must be a valid PostgreSQL connection URL",
        )
    })?;
    if config.get_ssl_mode() != SslMode::Disable {
        return Err(MigrationError::new(
            "database_tls_unsupported",
            "BIOHAZARDFS_DATABASE_URL must set sslmode=disable until server Postgres TLS support is implemented",
        ));
    }

    config.connect_timeout(Duration::from_secs(3));
    config.connect(NoTls).map_err(|_| {
        MigrationError::new(
            "database_unavailable",
            "Could not connect to the configured PostgreSQL database",
        )
    })
}

fn validate_database_url(database_url: Option<&str>) -> Result<&str, MigrationError> {
    let database_url = database_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            MigrationError::new(
                "database_url_missing",
                "BIOHAZARDFS_DATABASE_URL must be configured to run server migrations",
            )
        })?;

    if !(database_url.starts_with("postgres://") || database_url.starts_with("postgresql://")) {
        return Err(MigrationError::new(
            "database_url_invalid",
            "BIOHAZARDFS_DATABASE_URL must be a postgres:// or postgresql:// URL",
        ));
    }

    Ok(database_url)
}

fn migration_error_with_version(migration: &Migration, phase: &'static str) -> MigrationError {
    MigrationError::with_details(
        "migration_failed",
        "Could not apply one or more server database migrations",
        serde_json::json!({
            "migration_version": migration.version,
            "migration_name": migration.name,
            "phase": phase,
        }),
    )
}

fn migrations() -> &'static [Migration] {
    &[
        Migration {
            version: "001",
            name: "baseline",
            sql: include_str!("../migrations/001_baseline.sql"),
        },
        Migration {
            version: "002",
            name: "token_secret_hash_unique",
            sql: include_str!("../migrations/002_token_secret_hash_unique.sql"),
        },
        Migration {
            version: "003",
            name: "metadata_baseline",
            sql: include_str!("../migrations/003_metadata_baseline.sql"),
        },
    ]
}

impl Migration {
    fn summary(&self) -> MigrationSummary {
        MigrationSummary {
            version: self.version.to_string(),
            name: self.name.to_string(),
            checksum: checksum_sql(self.sql),
        }
    }
}

fn checksum_sql(sql: &str) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in sql.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("fnv1a64:{hash:016x}")
}

pub fn serve(addr: &str) -> std::io::Result<()> {
    serve_with_config(addr, RuntimeConfig::from_env())
}

pub fn serve_with_config(addr: &str, config: RuntimeConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let active_connections = Arc::new(AtomicUsize::new(0));
    let config = Arc::new(config);
    eprintln!("biohazardfs-server listening on http://{addr}");

    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let current = active_connections.load(Ordering::Relaxed);
                if current >= MAX_CONCURRENT_CONNECTIONS {
                    if let Err(error) = write_unavailable_response(&mut stream) {
                        eprintln!("biohazardfs-server overload response error: {error}");
                    }
                    continue;
                }

                active_connections.fetch_add(1, Ordering::Relaxed);
                let active_connections_for_thread = Arc::clone(&active_connections);
                let config_for_thread = Arc::clone(&config);
                let spawn_result = std::thread::Builder::new()
                    .name("biohazardfs-server-http".to_string())
                    .spawn(move || {
                        if let Err(error) = handle_stream(stream, &config_for_thread) {
                            eprintln!("biohazardfs-server request error: {error}");
                        }
                        active_connections_for_thread.fetch_sub(1, Ordering::Relaxed);
                    });

                if let Err(error) = spawn_result {
                    active_connections.fetch_sub(1, Ordering::Relaxed);
                    eprintln!("biohazardfs-server spawn error: {error}");
                }
            }
            Err(error) => eprintln!("biohazardfs-server accept error: {error}"),
        }
    }

    Ok(())
}

fn write_unavailable_response(stream: &mut TcpStream) -> std::io::Result<()> {
    let (_status_code, body) = json_response(
        503,
        &ServerResponseEnvelope::<serde_json::Value>::error(
            "server.request",
            ApiError::new("server_busy", "server scaffold connection limit reached"),
            Source::Server,
        ),
    );
    write_http_response(stream, 503, &body)
}

fn handle_stream(mut stream: TcpStream, config: &RuntimeConfig) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(1200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(1200)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let request_line = read_limited_line(&mut reader, MAX_REQUEST_LINE_BYTES)?;
    let mut saw_end_headers = false;
    let mut headers = Vec::new();

    for _ in 0..MAX_HEADERS {
        let header = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES)?;
        let header = header.trim_end();
        if header.is_empty() {
            saw_end_headers = true;
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }

    if !saw_end_headers {
        let (_status_code, body) = json_response(
            431,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new("too_many_headers", "server request has too many headers"),
                Source::Server,
            ),
        );
        return write_http_response(&mut stream, 431, &body);
    }

    let method = request_line.split_whitespace().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();

    if !matches!(method, "GET" | "PUT" | "POST" | "DELETE") {
        let (_status_code, body) = json_response(
            405,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new(
                    "method_not_allowed",
                    "server accepts GET, POST, DELETE, and bounded PUT requests",
                ),
                Source::Server,
            ),
        );
        return write_http_response(&mut stream, 405, &body);
    }

    let (route_path, _) = split_path_and_query(path);
    let should_read_body = (method == "PUT"
        && matches!(
            route_path,
            "/api/v1/objects/content" | "/api/v1/files/content"
        ))
        || (method == "POST" && matches!(route_path, "/api/v1/locks" | "/api/v1/operations"));
    let body_bytes = if should_read_body {
        reader
            .get_mut()
            .set_read_timeout(Some(Duration::from_secs(10)))?;
        match read_bounded_request_body(&mut reader, &headers) {
            Ok(body) => body,
            Err((status_code, error)) => {
                let (_status_code, body) = json_response(
                    status_code,
                    &ServerResponseEnvelope::<serde_json::Value>::error(
                        "server.request",
                        error,
                        Source::Server,
                    ),
                );
                return write_http_response(&mut stream, status_code, &body);
            }
        }
    } else {
        Vec::new()
    };

    let (status_code, body) =
        dispatch_http_request_with_config(method, path, &headers, &body_bytes, config);
    write_http_response(&mut stream, status_code, &body)
}

fn read_bounded_request_body(
    reader: &mut BufReader<TcpStream>,
    headers: &[(String, String)],
) -> Result<Vec<u8>, (u16, ApiError)> {
    let content_length = header_value(headers, "content-length")
        .ok_or_else(|| {
            (
                411,
                ApiError::new(
                    "content_length_required",
                    "PUT requests require Content-Length",
                ),
            )
        })?
        .parse::<usize>()
        .map_err(|_| {
            (
                400,
                ApiError::new("content_length_invalid", "Content-Length is not valid"),
            )
        })?;
    if content_length > MAX_CONTENT_UPLOAD_BYTES {
        return Err((
            413,
            ApiError::new(
                "content_too_large",
                "request body exceeds the MVP content upload limit",
            ),
        ));
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).map_err(|_| {
        (
            400,
            ApiError::new("body_read_failed", "could not read request body"),
        )
    })?;
    Ok(body)
}

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn json_response<T>(status_code: u16, envelope: &ServerResponseEnvelope<T>) -> (u16, String)
where
    T: serde::Serialize,
{
    let body = serde_json::to_string(envelope).unwrap_or_else(|error| {
        serde_json::json!({
            "ok": false,
            "operation": "server.serialize",
            "data": null,
            "warnings": [],
            "error": {"code": "serialization_error", "message": error.to_string(), "details": null},
            "meta": {"request_id": "req_serialize_error", "timestamp": "1970-01-01T00:00:00Z", "source": "server", "schema_version": SERVER_SCHEMA_VERSION, "api_version": "v1"}
        })
        .to_string()
    });
    (status_code, body)
}

fn write_http_response(
    stream: &mut TcpStream,
    status_code: u16,
    body: &str,
) -> std::io::Result<()> {
    let reason = reason_phrase(status_code);
    write!(
        stream,
        "HTTP/1.1 {status_code} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    stream.flush()
}

fn reason_phrase(status_code: u16) -> &'static str {
    match status_code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        411 => "Length Required",
        413 => "Payload Too Large",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    }
}

fn read_limited_line(
    reader: &mut BufReader<TcpStream>,
    max_bytes: usize,
) -> std::io::Result<String> {
    let mut bytes = Vec::new();
    loop {
        if bytes.len() >= max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "HTTP line exceeds scaffold limit",
            ));
        }

        let mut byte = [0_u8; 1];
        let read = reader.read(&mut byte)?;
        if read == 0 {
            break;
        }
        bytes.push(byte[0]);
        if byte[0] == b'\n' {
            break;
        }
    }

    String::from_utf8(bytes).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("HTTP line is not valid UTF-8: {error}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_is_server_ready() {
        let status = server_status("serve");
        assert_eq!(status.name, "biohazardfs-server");
        assert_eq!(status.state, ServerState::Ready);
        assert_eq!(status.api_version, "v1");
    }

    #[test]
    fn dispatch_healthz_uses_server_envelope() {
        let (status_code, body) = dispatch_http_path("/healthz");
        assert_eq!(status_code, 200);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["operation"], "server.health");
        assert_eq!(value["meta"]["schema_version"], SERVER_SCHEMA_VERSION);
    }

    #[test]
    fn dispatch_readyz_uses_server_envelope() {
        let (status_code, body) = dispatch_http_path("/readyz");
        assert!(matches!(status_code, 200 | 503));
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["operation"], "server.ready");
        assert_eq!(value["meta"]["schema_version"], SERVER_SCHEMA_VERSION);
    }

    #[test]
    fn dispatch_unknown_path_returns_not_found_envelope() {
        let (status_code, body) = dispatch_http_path("/missing");
        assert_eq!(status_code, 404);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "not_found");
    }

    #[test]
    fn migrate_without_database_url_returns_secret_safe_error() {
        let error = migrate_with_database_url(None).expect_err("database URL is required");
        assert_eq!(error.code(), "database_url_missing");
        let envelope = ServerResponseEnvelope::<serde_json::Value>::error(
            "server.migrate",
            error.into_api_error(),
            Source::Server,
        );
        let text = serde_json::to_string(&envelope).expect("envelope serializes");
        assert!(text.contains("database_url_missing"));
        assert!(!text.contains("postgres://"));
        assert!(!text.contains("password"));
    }

    #[test]
    fn database_url_requires_explicit_plaintext_mode_until_tls_lands() {
        let error = migrate_with_database_url(Some("postgres://user:password@example/db"))
            .expect_err("implicit plaintext database URLs are rejected before connect");
        assert_eq!(error.code(), "database_tls_unsupported");
        let text = serde_json::to_string(&error.into_api_error()).expect("error serializes");
        assert!(!text.contains("password"));
        assert!(!text.contains("example"));
    }

    #[test]
    fn invalid_database_url_error_does_not_echo_secret() {
        let error = migrate_with_database_url(Some("postgresql+secret://user:password@example/db"))
            .expect_err("invalid database URL is rejected before connect");
        let text = serde_json::to_string(&error.into_api_error()).expect("error serializes");
        assert!(text.contains("database_url_invalid"));
        assert!(!text.contains("postgresql+secret"));
        assert!(!text.contains("password"));
    }

    #[test]
    fn bearer_token_hash_is_stable_and_secret_safe() {
        let hash = secret_hash_for_token("smoke-token");
        assert_eq!(
            hash,
            "sha256:811fc2dd0a6d4649f89e06cdf61ec8633f709140bcda5a1f56d724cbd548e014"
        );
        assert!(!hash.contains("smoke-token"));
    }

    #[test]
    fn namespace_scope_parser_requires_read_scope() {
        assert!(scopes_allow_namespace_read(r#"["namespace:read"]"#));
        assert!(scopes_allow_namespace_read(r#"["*"]"#));
        assert!(!scopes_allow_namespace_read(r#"["file:write"]"#));
        assert!(!scopes_allow_namespace_read("not-json"));
    }

    #[test]
    fn object_scope_parser_separates_read_and_write() {
        assert!(scopes_allow_object_read(r#"["object:read"]"#));
        assert!(scopes_allow_object_read(r#"["file:read"]"#));
        assert!(scopes_allow_object_write(r#"["object:write"]"#));
        assert!(scopes_allow_object_write(r#"["file:write"]"#));
        assert!(scopes_allow_object_read(r#"["*"]"#));
        assert!(scopes_allow_object_write(r#"["*"]"#));
        assert!(!scopes_allow_object_read(r#"["object:write"]"#));
        assert!(!scopes_allow_object_write(r#"["object:read"]"#));
    }

    #[test]
    fn namespace_query_parses_parent_and_limit() {
        let query = parse_namespace_children_query("parent=root&limit=3").expect("valid query");
        assert_eq!(query.parent_node_id.as_deref(), Some("root"));
        assert_eq!(query.limit, 3);
    }

    #[test]
    fn object_store_endpoint_parses_path_style_rustfs_url() {
        let endpoint =
            parse_object_store_endpoint("http://object-store:9000").expect("valid endpoint");
        assert_eq!(endpoint.host, "object-store");
        assert_eq!(endpoint.host_header, "object-store:9000");
        assert_eq!(endpoint.port, 9000);
        assert_eq!(
            object_store_bucket_uri(&endpoint, "biohazardfs-dev"),
            "/biohazardfs-dev"
        );
    }

    #[test]
    fn object_store_request_uri_includes_content_object_key() {
        let config = RuntimeConfig::from_lookup(|key| match key {
            biohazardfs_core::config::ENV_OBJECT_STORE_ENDPOINT => {
                Some("http://object-store:9000".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_BUCKET => {
                Some("biohazardfs-dev".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_ACCESS_KEY_ID => {
                Some("biohazardfs".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_SECRET_ACCESS_KEY => {
                Some("dev-secret".to_string())
            }
            _ => None,
        });
        let request = ObjectStoreRequest::from_config(&config, "GET")
            .and_then(|request| {
                request.with_object_payload(
                    "orgs/org_smoke/content/sha256/abc123".to_string(),
                    EMPTY_PAYLOAD_SHA256.to_string(),
                    0,
                )
            })
            .expect("request builds");
        assert_eq!(
            object_store_request_uri(&request),
            "/biohazardfs-dev/orgs/org_smoke/content/sha256/abc123"
        );
    }

    #[test]
    fn object_store_endpoint_rejects_https_until_tls_lands() {
        let error = parse_object_store_endpoint("https://object-store.example")
            .expect_err("TLS object-store client is not implemented yet");
        assert_eq!(error.code(), "object_store_endpoint_unsupported");
    }

    #[test]
    fn object_store_endpoint_rejects_public_cleartext_hosts() {
        let error = parse_object_store_endpoint("http://object-store.example.com:9000")
            .expect_err("public cleartext object-store endpoints are unsafe");
        assert_eq!(error.code(), "object_store_endpoint_insecure");
        assert!(parse_object_store_endpoint("http://192.168.1.128:9000").is_ok());
        assert_eq!(
            parse_object_store_endpoint("http://169.254.169.254:9000")
                .expect_err("link-local metadata endpoints are not safe object-store targets")
                .code(),
            "object_store_endpoint_insecure"
        );
        assert!(parse_object_store_endpoint("http://object-store:9000").is_ok());
        assert!(
            parse_object_store_endpoint("http://rustfs.storage.svc.cluster.local:9000").is_ok()
        );
    }

    #[test]
    fn object_store_bucket_validation_blocks_path_injection() {
        assert!(validate_bucket_name("biohazardfs-dev").is_ok());
        assert_eq!(
            validate_bucket_name("../biohazardfs-dev")
                .expect_err("path-like bucket name is invalid")
                .code(),
            "object_store_bucket_invalid"
        );
        assert_eq!(
            validate_bucket_name("BiohazardFS")
                .expect_err("uppercase bucket name is invalid")
                .code(),
            "object_store_bucket_invalid"
        );
    }

    #[test]
    fn object_store_endpoint_rejects_header_injection() {
        let error = parse_object_store_endpoint("http://object-store:9000/good\nInjected: nope")
            .expect_err("control characters are invalid");
        assert_eq!(error.code(), "object_store_endpoint_invalid");
    }

    #[test]
    fn object_store_signing_fields_reject_header_injection() {
        assert!(validate_access_key_id("BHFSOBJECTSMOKE_123").is_ok());
        assert!(validate_region("us-east-1").is_ok());
        assert_eq!(
            validate_access_key_id("key\nInjected: nope")
                .expect_err("control characters are invalid")
                .code(),
            "object_store_access_key_invalid"
        );
        assert_eq!(
            validate_region("us-east-1\r\nInjected")
                .expect_err("control characters are invalid")
                .code(),
            "object_store_region_invalid"
        );
    }

    #[test]
    fn object_store_request_uses_redacted_config_secrets_internally() {
        let config = RuntimeConfig::from_lookup(|key| match key {
            biohazardfs_core::config::ENV_OBJECT_STORE_ENDPOINT => {
                Some("http://object-store:9000".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_BUCKET => {
                Some("biohazardfs-dev".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_ACCESS_KEY_ID => {
                Some("biohazardfs".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_SECRET_ACCESS_KEY => {
                Some("dev-secret".to_string())
            }
            _ => None,
        });
        let serialized = serde_json::to_string(&config).expect("config serializes");
        assert!(serialized.contains("access_key_id_set"));
        assert!(!serialized.contains("biohazardfs\""));
        assert!(!serialized.contains("dev-secret"));
        let request = ObjectStoreRequest::from_config(&config, "HEAD").expect("request builds");
        assert_eq!(request.access_key_id, "biohazardfs");
        assert_eq!(request.secret_access_key, "dev-secret");
    }

    #[test]
    fn bundled_migration_has_required_mvp_tables() {
        let sql = migrations()[0].sql;
        for table in [
            "organizations",
            "users",
            "tokens",
            "nodes",
            "content_manifests",
            "file_versions",
            "operations",
            "upload_sessions",
            "audit_events",
        ] {
            let needle = format!("CREATE TABLE {table}");
            assert!(sql.contains(&needle), "missing {table}");
        }
        assert!(
            SCHEMA_MIGRATIONS_TABLE_SQL.contains("CREATE TABLE IF NOT EXISTS schema_migrations")
        );
        assert!(sql.contains("secret_hash TEXT NOT NULL"));
        assert!(
            migrations()[1]
                .sql
                .contains("CREATE UNIQUE INDEX tokens_secret_hash_unique")
        );
        assert!(!sql.contains("raw_token"));
    }

    #[test]
    fn unknown_applied_migration_versions_are_rejected() {
        let mut applied = std::collections::BTreeMap::new();
        applied.insert(
            "999".to_string(),
            AppliedMigration {
                version: "999".to_string(),
                name: "future".to_string(),
                checksum: "fnv1a64:future".to_string(),
            },
        );

        let error = verify_no_unknown_applied_migrations(&applied)
            .expect_err("future migration versions should fail");
        assert_eq!(error.code(), "migration_version_unsupported");
    }

    #[test]
    fn migration_summary_reports_current_version_without_database() {
        let summary = migrations()[0].summary();
        assert_eq!(summary.version, "001");
        assert_eq!(summary.name, "baseline");
        assert!(summary.checksum.starts_with("fnv1a64:"));
    }

    #[test]
    fn migration_003_is_registered_with_expected_identity() {
        let migration = migrations()
            .iter()
            .find(|migration| migration.version == "003")
            .expect("migration 003 is registered");
        assert_eq!(migration.name, "metadata_baseline");
        assert!(migration.summary().checksum.starts_with("fnv1a64:"));
        assert_eq!(migrations().len(), 3);
    }

    #[test]
    fn migration_003_has_required_metadata_tables() {
        let sql = migrations()
            .iter()
            .find(|migration| migration.version == "003")
            .expect("migration 003 is registered")
            .sql;
        for table in [
            "devices",
            "projects",
            "worksets",
            "workset_rules",
            "retention_policies",
            "snapshots",
            "locks",
            "conflicts",
            "grants",
            "shares",
            "publishes",
            "invites",
            "trash_records",
        ] {
            let needle = format!("CREATE TABLE {table}");
            assert!(sql.contains(&needle), "migration 003 missing {table}");
        }
        // New tables stay org-scoped like 001.
        assert!(sql.contains("REFERENCES organizations(org_id) ON DELETE RESTRICT"));
        // Snapshots reuse the lowercase status text convention from 001.
        assert!(
            sql.contains("CHECK (status IN ('creating', 'ready', 'failed', 'expired', 'purged'))")
        );
        // Locks keep the node_id optional for offline provisional IDs.
        assert!(sql.contains("node_id TEXT,\n    provisional_local_id TEXT"));
        // No raw secrets or DOWN section in the new migration.
        assert!(!sql.contains("password"));
        assert!(!sql.contains("secret"));
    }

    fn no_database_config() -> RuntimeConfig {
        // from_lookup never reads config files; returning None for every env
        // key yields database.url_set = false and no object-store endpoint,
        // so authenticated spine routes fail closed at the connect step.
        RuntimeConfig::from_lookup(|_| None)
    }

    fn dispatch_with(
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> (u16, String) {
        dispatch_http_request_with_config(method, path, headers, body, &no_database_config())
    }

    fn assert_server_envelope(ok: bool, body: &str, operation: &str) -> serde_json::Value {
        let value: serde_json::Value = serde_json::from_str(body).expect("valid json");
        assert_eq!(value["ok"], ok, "operation {operation}: {body}");
        assert_eq!(
            value["operation"], operation,
            "operation {operation}: {body}"
        );
        assert_eq!(
            value["meta"]["schema_version"], SERVER_SCHEMA_VERSION,
            "operation {operation}: {body}"
        );
        value
    }

    #[test]
    fn dispatch_locks_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/locks", &[], &[]);
        assert_eq!(status, 401);
        let value = assert_server_envelope(false, &body, "server.locks.list");
        assert_eq!(value["error"]["code"], "auth_required");
    }

    #[test]
    fn dispatch_locks_list_with_bearer_fails_closed_without_database() {
        let headers = vec![(
            "Authorization".to_string(),
            "Bearer smoke-lock-token".to_string(),
        )];
        let (status, body) = dispatch_with("GET", "/api/v1/locks", &headers, &[]);
        assert_eq!(status, 503);
        let value = assert_server_envelope(false, &body, "server.locks.list");
        // Auth runs before any store write, so the bearer secret is never echoed.
        assert_eq!(value["error"]["code"], "database_url_missing");
        assert!(!body.contains("smoke-lock-token"));
    }

    #[test]
    fn dispatch_locks_acquire_requires_bearer_after_body_parse() {
        let body = br#"{"kind":"edit"}"#;
        let (status, body) = dispatch_with("POST", "/api/v1/locks", &[], body);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.locks.acquire");
    }

    #[test]
    fn dispatch_locks_acquire_rejects_invalid_kind_before_auth() {
        let body = br#"{"kind":"nope"}"#;
        let (status, body) = dispatch_with("POST", "/api/v1/locks", &[], body);
        assert_eq!(status, 400);
        let value = assert_server_envelope(false, &body, "server.locks.acquire");
        assert_eq!(value["error"]["code"], "invalid_lock_kind");
    }

    #[test]
    fn dispatch_locks_acquire_rejects_oversized_ttl() {
        let body = br#"{"kind":"edit","ttl_seconds":99999999}"#;
        let (status, _body) = dispatch_with("POST", "/api/v1/locks", &[], body);
        assert_eq!(status, 400);
    }

    #[test]
    fn dispatch_locks_release_requires_lock_id() {
        let (status, body) = dispatch_with("DELETE", "/api/v1/locks", &[], &[]);
        assert_eq!(status, 400);
        let value = assert_server_envelope(false, &body, "server.locks.release");
        assert_eq!(value["error"]["code"], "lock_id_required");
    }

    #[test]
    fn dispatch_locks_release_requires_bearer_with_lock_id() {
        let (status, body) = dispatch_with("DELETE", "/api/v1/locks?lock_id=lock_abc", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.locks.release");
    }

    #[test]
    fn dispatch_conflicts_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/conflicts", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.conflicts.list");
    }

    #[test]
    fn dispatch_conflicts_show_routes_by_conflict_id() {
        let (status, body) =
            dispatch_with("GET", "/api/v1/conflicts?conflict_id=conf_xyz", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.conflicts.show");
    }

    #[test]
    fn dispatch_operations_submit_requires_bearer_after_body_parse() {
        let body = br#"{"kind":"file.write"}"#;
        let (status, body) = dispatch_with("POST", "/api/v1/operations", &[], body);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.operations.submit");
    }

    #[test]
    fn dispatch_operations_submit_rejects_invalid_kind() {
        let body = br#"{"kind":"bad kind"}"#;
        let (status, body) = dispatch_with("POST", "/api/v1/operations", &[], body);
        assert_eq!(status, 400);
        let value = assert_server_envelope(false, &body, "server.operations.submit");
        assert_eq!(value["error"]["code"], "invalid_operation_kind");
    }

    #[test]
    fn dispatch_trash_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/trash", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.trash.list");
    }

    #[test]
    fn dispatch_audit_events_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/audit/events", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.audit.events");
    }

    #[test]
    fn dispatch_devices_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/devices", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.devices.list");
    }

    #[test]
    fn dispatch_projects_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/projects", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.projects.list");
    }

    #[test]
    fn dispatch_worksets_list_requires_bearer() {
        let (status, body) = dispatch_with("GET", "/api/v1/worksets", &[], &[]);
        assert_eq!(status, 401);
        assert_server_envelope(false, &body, "server.worksets.list");
    }

    #[test]
    fn periphery_routes_return_operation_not_implemented() {
        for (method, path, operation) in [
            ("POST", "/api/v1/snapshots", "server.snapshots.create"),
            ("POST", "/api/v1/snapshots/mount", "server.snapshots.mount"),
            (
                "POST",
                "/api/v1/snapshots/restore",
                "server.snapshots.restore",
            ),
            ("GET", "/api/v1/snapshots", "server.snapshots.list"),
            ("POST", "/api/v1/transfers", "server.transfers.create"),
            (
                "POST",
                "/api/v1/transfers/commit",
                "server.transfers.commit",
            ),
            ("POST", "/api/v1/devices/revoke", "server.devices.revoke"),
            ("POST", "/api/v1/projects", "server.projects.create"),
            ("POST", "/api/v1/worksets", "server.worksets.create"),
            ("POST", "/api/v1/trash/restore", "server.trash.restore"),
            ("POST", "/api/v1/trash/purge", "server.trash.purge"),
            (
                "POST",
                "/api/v1/operations/replay",
                "server.operations.replay",
            ),
            ("GET", "/api/v1/audit/export", "server.audit.export"),
            ("GET", "/api/v1/grants", "server.grants.list"),
            ("POST", "/api/v1/grants", "server.grants.set"),
            ("DELETE", "/api/v1/grants", "server.grants.revoke"),
            ("GET", "/api/v1/shares", "server.shares.list"),
            ("POST", "/api/v1/shares", "server.shares.create"),
            ("DELETE", "/api/v1/shares", "server.shares.revoke"),
            ("GET", "/api/v1/publishes", "server.publishes.list"),
            ("POST", "/api/v1/publishes", "server.publishes.create"),
            ("DELETE", "/api/v1/publishes", "server.publishes.revoke"),
            ("GET", "/api/v1/invites", "server.invites.list"),
            ("POST", "/api/v1/invites", "server.invites.create"),
            ("DELETE", "/api/v1/invites", "server.invites.revoke"),
            ("GET", "/api/v1/nodes/stat", "server.nodes.stat"),
            ("POST", "/api/v1/nodes/mkdir", "server.nodes.mkdir"),
            ("POST", "/api/v1/nodes/symlink", "server.nodes.symlink"),
            ("POST", "/api/v1/nodes/move", "server.nodes.move"),
            ("POST", "/api/v1/nodes/copy", "server.nodes.copy"),
            ("DELETE", "/api/v1/nodes", "server.nodes.delete"),
            (
                "POST",
                "/api/v1/auth/device/enroll",
                "server.auth.device.enroll",
            ),
            (
                "POST",
                "/api/v1/auth/login_token",
                "server.auth.login_token",
            ),
        ] {
            let (status, body) = dispatch_with(method, path, &[], &[]);
            assert_eq!(status, 501, "{method} {path}: {body}");
            let value = assert_server_envelope(false, &body, operation);
            assert_eq!(
                value["error"]["code"], "operation_not_implemented",
                "{method} {path}"
            );
        }
    }

    #[test]
    fn known_route_with_wrong_method_returns_method_not_allowed() {
        // GET on a POST-only collection is a 405, not a 404.
        let (status, body) = dispatch_with("GET", "/api/v1/operations", &[], &[]);
        assert_eq!(status, 405);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "method_not_allowed");
    }

    #[test]
    fn spine_scope_helpers_distinguish_read_and_write() {
        assert!(scopes_allow_lock_read(r#"["lock:read"]"#));
        assert!(scopes_allow_lock_write(r#"["lock:write"]"#));
        assert!(!scopes_allow_lock_write(r#"["lock:read"]"#));
        assert!(scopes_allow_conflict_read(r#"["conflict:read"]"#));
        assert!(scopes_allow_conflict_read(r#"["namespace:read"]"#));
        assert!(!scopes_allow_conflict_read(r#"["lock:read"]"#));
        assert!(scopes_allow_operation_write(r#"["operation:write"]"#));
        assert!(!scopes_allow_operation_write(r#"["operation:read"]"#));
        assert!(scopes_allow_trash_read(r#"["trash:read"]"#));
        assert!(scopes_allow_audit_read(r#"["audit:read"]"#));
        assert!(scopes_allow_devices_read(r#"["device:read"]"#));
        assert!(scopes_allow_projects_read(r#"["project:read"]"#));
        assert!(scopes_allow_worksets_read(r#"["workset:read"]"#));
        // Every scope helper honors the wildcard.
        assert!(scopes_allow_lock_read(r#"["*"]"#));
        assert!(scopes_allow_lock_write(r#"["*"]"#));
        assert!(scopes_allow_operation_write(r#"["*"]"#));
    }

    #[test]
    fn lock_acquire_body_defaults_edit_kind_and_ttl() {
        let body: LockAcquireBody = serde_json::from_str(r#"{}"#).expect("defaults apply");
        assert_eq!(body.kind, "edit");
        assert_eq!(body.ttl_seconds, DEFAULT_LOCK_TTL_SECONDS);
        assert!(body.node_id.is_none());
    }

    #[test]
    fn lock_ttl_seconds_enforces_upper_bound() {
        assert!(lock_ttl_seconds(0).is_ok());
        assert!(lock_ttl_seconds(MAX_LOCK_TTL_SECONDS).is_ok());
        let error = lock_ttl_seconds(MAX_LOCK_TTL_SECONDS + 1).expect_err("oversized ttl rejected");
        assert_eq!(error.1.code, "invalid_lock_ttl");
    }

    #[test]
    fn idempotency_key_validator_rejects_garbage() {
        assert!(validate_idempotency_key("abc_123-456").is_ok());
        // empty
        assert_eq!(
            validate_idempotency_key("").expect_err("empty key").1.code,
            "invalid_idempotency_key"
        );
        // disallowed characters
        assert_eq!(
            validate_idempotency_key("bad key!")
                .expect_err("bad charset")
                .1
                .code,
            "invalid_idempotency_key"
        );
    }

    #[test]
    fn operation_kind_validator_rejects_garbage() {
        assert!(validate_operation_kind("file.write").is_ok());
        assert_eq!(
            validate_operation_kind("").expect_err("empty kind").1.code,
            "invalid_operation_kind"
        );
        assert_eq!(
            validate_operation_kind("bad kind")
                .expect_err("spaces")
                .1
                .code,
            "invalid_operation_kind"
        );
    }

    #[test]
    fn list_limit_parser_enforces_bounds() {
        assert_eq!(parse_limit_value("1").expect("min"), 1);
        assert_eq!(parse_limit_value("100").expect("default"), 100);
        assert_eq!(
            parse_limit_value("0").expect_err("zero").1.code,
            "invalid_limit"
        );
        assert_eq!(
            parse_limit_value(&(MAX_LIST_LIMIT + 1).to_string())
                .expect_err("over max")
                .1
                .code,
            "invalid_limit",
            "limit above max is rejected"
        );
        // MAX_LIST_LIMIT itself must be allowed.
        assert_eq!(
            parse_limit_value(&MAX_LIST_LIMIT.to_string()).expect("max"),
            MAX_LIST_LIMIT
        );
    }

    #[test]
    fn opaque_id_validator_rejects_path_traversal() {
        assert!(validate_opaque_id("lock_abc123", "invalid_lock_id", "lock_id").is_ok());
        // path-like and injection characters are rejected before reaching SQL.
        assert_eq!(
            validate_opaque_id("../etc/passwd", "invalid_lock_id", "lock_id")
                .expect_err("path traversal")
                .1
                .code,
            "invalid_lock_id"
        );
        assert_eq!(
            validate_opaque_id("a'b OR 1=1", "invalid_lock_id", "lock_id")
                .expect_err("sql injection")
                .1
                .code,
            "invalid_lock_id"
        );
    }

    #[test]
    fn status_filter_validator_bounds_known_values() {
        assert!(validate_status_filter("active", &["active", "released"]).is_ok());
        assert_eq!(
            validate_status_filter("deleted", &["active", "released"])
                .expect_err("unknown status")
                .1
                .code,
            "invalid_status"
        );
    }

    #[test]
    fn admin_payload_is_redacted_and_secret_safe() {
        let config = RuntimeConfig::from_lookup(|key| match key {
            biohazardfs_core::config::ENV_DATABASE_URL => {
                Some("postgres://user:super-secret@db.example/biohazard".to_string())
            }
            biohazardfs_core::config::ENV_OBJECT_STORE_SECRET_ACCESS_KEY => {
                Some("object-store-secret".to_string())
            }
            _ => None,
        });
        let report = admin_payload(&config);
        assert!(report.database_configured);
        assert_eq!(report.mode, "admin");
        let text = serde_json::to_string(&report).expect("admin report serializes");
        assert!(text.contains("database_configured"));
        assert!(!text.contains("super-secret"));
        assert!(!text.contains("object-store-secret"));
        assert!(!text.contains("db.example"));
    }

    #[test]
    fn admin_payload_reports_unconfigured_dependencies() {
        let report = admin_payload(&no_database_config());
        assert!(!report.database_configured);
        assert!(!report.object_store_configured);
    }

    #[test]
    fn known_server_route_table_covers_wave2_paths() {
        // The route table must honestly register every Wave 2 path so the
        // dispatch returns 501 (periphery) or a real envelope (spine) instead
        // of a 404 for these documented operations.
        for path in [
            "/api/v1/locks",
            "/api/v1/conflicts",
            "/api/v1/operations",
            "/api/v1/operations/replay",
            "/api/v1/trash",
            "/api/v1/trash/restore",
            "/api/v1/trash/purge",
            "/api/v1/audit/events",
            "/api/v1/audit/export",
            "/api/v1/devices",
            "/api/v1/devices/revoke",
            "/api/v1/projects",
            "/api/v1/worksets",
            "/api/v1/snapshots",
            "/api/v1/snapshots/mount",
            "/api/v1/snapshots/restore",
            "/api/v1/transfers",
            "/api/v1/transfers/commit",
            "/api/v1/grants",
            "/api/v1/shares",
            "/api/v1/publishes",
            "/api/v1/invites",
            "/api/v1/nodes",
            "/api/v1/nodes/stat",
            "/api/v1/nodes/mkdir",
            "/api/v1/nodes/symlink",
            "/api/v1/nodes/move",
            "/api/v1/nodes/copy",
            "/api/v1/auth/device/enroll",
            "/api/v1/auth/login_token",
        ] {
            assert!(is_known_server_route(path), "route table missing {path}");
        }
        assert!(!is_known_server_route("/api/v1/missing"));
        assert!(!is_known_server_route("/api/v1/locks/extra"));
    }

    fn unique_live_suffix() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        // Numeric-only suffix is safe to interpolate into seed SQL; it cannot
        // carry SQL metacharacters.
        format!(
            "{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        )
    }

    fn live_db_url() -> Option<String> {
        let url = std::env::var("BIOHAZARDFS_TEST_DATABASE_URL").ok()?;
        // The synchronous Postgres client requires explicit plaintext mode until
        // server TLS support lands; refuse anything else so the test fails
        // loudly instead of producing a confusing connect error.
        assert!(
            url.contains("sslmode=disable"),
            "BIOHAZARDFS_TEST_DATABASE_URL must set sslmode=disable"
        );
        Some(url)
    }

    fn live_test_config(database_url: &str) -> RuntimeConfig {
        RuntimeConfig::from_lookup(|key| {
            if key == biohazardfs_core::config::ENV_DATABASE_URL {
                Some(database_url.to_string())
            } else {
                None
            }
        })
    }

    /// Live-Postgres integration check for the Wave 2 spine. Ignored by default
    /// because it needs a real database; run it with:
    ///
    /// `BIOHAZARDFS_TEST_DATABASE_URL='postgres://...?sslmode=disable' cargo test -p biohazardfs-server -- --ignored`
    ///
    /// The scripts/ci/server-db-smoke.sh harness is the canonical live
    /// coverage; this test exists so a developer can reproduce the exact
    /// safety invariants cargo cannot see: only active locks release, missing
    /// referenced nodes are rejected, idempotent operation replay returns the
    /// same operation id, and scope enforcement holds through the full stack.
    /// The SQL round-trip also exercises the to_char/make_interval/jsonb-cast
    /// expressions used by the spine.
    #[test]
    #[ignore]
    fn live_db_spine_routes_and_safety_invariants() {
        let Some(database_url) = live_db_url() else {
            eprintln!(
                "skipping live DB test: set BIOHAZARDFS_TEST_DATABASE_URL to a sslmode=disable Postgres URL"
            );
            return;
        };
        run_migrations(&database_url).expect("apply bundled migrations");
        let mut client = connect_database(&database_url).expect("connect to test database");

        let suffix = unique_live_suffix();
        let org_id = format!("org_live_{suffix}");
        let user_id = format!("usr_live_{suffix}");
        let dir_node = format!("node_dir_{suffix}");
        let file_node = format!("node_file_{suffix}");
        let admin_token_id = format!("tok_admin_{suffix}");
        let reader_token_id = format!("tok_reader_{suffix}");
        let admin_bearer = format!("live-admin-{suffix}");
        let reader_bearer = format!("live-reader-{suffix}");
        let admin_hash = secret_hash_for_token(&admin_bearer);
        let reader_hash = secret_hash_for_token(&reader_bearer);
        let admin_scopes = r#"["*"]"#;
        let reader_scopes = r#"["lock:read"]"#;

        client
            .execute(
                "INSERT INTO organizations (org_id, slug, display_name, status)
                 VALUES ($1, $2, $3, 'active')",
                &[&org_id, &org_id, &org_id],
            )
            .expect("seed organization");
        client
            .execute(
                "INSERT INTO users (org_id, user_id, display_name, email, status)
                 VALUES ($1, $2, $3, $4, 'active')",
                &[
                    &org_id,
                    &user_id,
                    &user_id,
                    &format!("live-{suffix}@local.invalid"),
                ],
            )
            .expect("seed user");
        client
            .execute(
                "INSERT INTO nodes (org_id, node_id, parent_node_id, name, kind, owner_user_id, created_by, updated_by)
                 VALUES ($1, $2, NULL, 'dir', 'directory', $3, $3, $3)",
                &[&org_id, &dir_node, &user_id],
            )
            .expect("seed directory node");
        client
            .execute(
                "INSERT INTO nodes (org_id, node_id, parent_node_id, name, kind, owner_user_id, created_by, updated_by)
                 VALUES ($1, $2, $3, 'file.txt', 'file', $4, $4, $4)",
                &[&org_id, &file_node, &dir_node, &user_id],
            )
            .expect("seed file node");
        client
            .execute(
                "INSERT INTO tokens (org_id, token_id, user_id, kind, scopes, status, secret_hash)
                 VALUES ($1, $2, $3, 'api', $4::text::jsonb, 'active', $5)",
                &[
                    &org_id,
                    &admin_token_id,
                    &user_id,
                    &admin_scopes,
                    &admin_hash,
                ],
            )
            .expect("seed admin token");
        client
            .execute(
                "INSERT INTO tokens (org_id, token_id, user_id, kind, scopes, status, secret_hash)
                 VALUES ($1, $2, $3, 'api', $4::text::jsonb, 'active', $5)",
                &[
                    &org_id,
                    &reader_token_id,
                    &user_id,
                    &reader_scopes,
                    &reader_hash,
                ],
            )
            .expect("seed reader token");

        let config = live_test_config(&database_url);
        let admin_auth = vec![(
            "Authorization".to_string(),
            format!("Bearer {admin_bearer}"),
        )];
        let reader_auth = vec![(
            "Authorization".to_string(),
            format!("Bearer {reader_bearer}"),
        )];

        // Acquire returns RFC3339 UTC timestamps and an active lock.
        let acquire_body =
            format!(r#"{{"node_id":"{file_node}","kind":"edit","ttl_seconds":300}}"#);
        let (status, body) = dispatch_http_request_with_config(
            "POST",
            "/api/v1/locks",
            &admin_auth,
            acquire_body.as_bytes(),
            &config,
        );
        assert_eq!(status, 200, "acquire should succeed: {body}");
        let value: serde_json::Value = serde_json::from_str(&body).expect("envelope json");
        assert_eq!(value["operation"], "server.locks.acquire");
        assert_eq!(value["data"]["status"], "active");
        assert!(
            value["data"]["acquired_at"]
                .as_str()
                .is_some_and(|ts| ts.ends_with('Z'))
        );
        assert!(
            !body.contains(&admin_bearer),
            "envelope must not echo the bearer"
        );
        let lock_id = value["data"]["lock_id"].as_str().unwrap().to_string();

        // List contains the just-acquired lock.
        let (status, body) =
            dispatch_http_request_with_config("GET", "/api/v1/locks", &admin_auth, &[], &config);
        assert_eq!(status, 200);
        assert!(
            body.contains(&lock_id),
            "list must contain the acquired lock"
        );

        // Release transitions to released.
        let (status, body) = dispatch_http_request_with_config(
            "DELETE",
            &format!("/api/v1/locks?lock_id={lock_id}"),
            &admin_auth,
            &[],
            &config,
        );
        assert_eq!(status, 200);
        assert!(body.contains("\"released\""));

        // SAFETY: a second release of a now-non-active lock is a 404, never a
        // silent success.
        let (status, body) = dispatch_http_request_with_config(
            "DELETE",
            &format!("/api/v1/locks?lock_id={lock_id}"),
            &admin_auth,
            &[],
            &config,
        );
        assert_eq!(status, 404, "double release must be 404: {body}");

        // SAFETY: acquiring against a missing node is rejected before insert.
        let (status, _body) = dispatch_http_request_with_config(
            "POST",
            "/api/v1/locks",
            &admin_auth,
            br#"{"node_id":"node_missing","kind":"edit"}"#,
            &config,
        );
        assert_eq!(status, 404, "acquire against missing node must be 404");

        // Idempotent operation replay returns the same operation id and the
        // payload_json round-trips as valid jsonb.
        let op_body = format!(
            r#"{{"kind":"file.write","node_id":"{file_node}","idempotency_key":"idem-{suffix}","params":{{"foo":"bar"}}}}"#
        );
        let (status, body) = dispatch_http_request_with_config(
            "POST",
            "/api/v1/operations",
            &admin_auth,
            op_body.as_bytes(),
            &config,
        );
        assert_eq!(status, 200, "submit should succeed: {body}");
        let op_id1 =
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["operation_id"]
                .as_str()
                .unwrap()
                .to_string();
        let (status, body) = dispatch_http_request_with_config(
            "POST",
            "/api/v1/operations",
            &admin_auth,
            op_body.as_bytes(),
            &config,
        );
        assert_eq!(status, 200);
        let op_id2 =
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["data"]["operation_id"]
                .as_str()
                .unwrap()
                .to_string();
        assert_eq!(
            op_id1, op_id2,
            "idempotent replay must return the same operation id"
        );

        // Read endpoints return org-scoped 200 envelopes with the contract shape.
        for (method, path, operation, data_key) in [
            (
                "GET",
                "/api/v1/conflicts",
                "server.conflicts.list",
                "conflicts",
            ),
            ("GET", "/api/v1/trash", "server.trash.list", "trash"),
            (
                "GET",
                "/api/v1/audit/events",
                "server.audit.events",
                "events",
            ),
            ("GET", "/api/v1/devices", "server.devices.list", "devices"),
            (
                "GET",
                "/api/v1/projects",
                "server.projects.list",
                "projects",
            ),
            (
                "GET",
                "/api/v1/worksets",
                "server.worksets.list",
                "worksets",
            ),
        ] {
            let (status, body) =
                dispatch_http_request_with_config(method, path, &admin_auth, &[], &config);
            assert_eq!(status, 200, "{method} {path}: {body}");
            let value: serde_json::Value = serde_json::from_str(&body).expect("envelope json");
            assert_eq!(value["operation"], operation);
            assert!(
                value["data"][data_key].is_array(),
                "{operation}: {data_key} must be an array"
            );
        }

        // Scope enforcement through the full stack: a read-only token can list
        // locks but cannot acquire one.
        let (status, _body) =
            dispatch_http_request_with_config("GET", "/api/v1/locks", &reader_auth, &[], &config);
        assert_eq!(status, 200, "reader token can list locks");
        let (status, body) = dispatch_http_request_with_config(
            "POST",
            "/api/v1/locks",
            &reader_auth,
            br#"{"kind":"edit"}"#,
            &config,
        );
        assert_eq!(status, 403, "reader token cannot acquire: {body}");
        assert!(body.contains("auth_scope_missing"));
    }
}
