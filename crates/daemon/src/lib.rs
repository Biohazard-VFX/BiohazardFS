//! BiohazardFS local daemon (DAEMON_API.md).
//!
//! The scaffold exposes a JSON-RPC-like dispatch over the dev-loopback HTTP
//! transport. All state lives in [`DaemonBackend`] (in-memory mock); the
//! dispatch table wires every method in `known_methods::DAEMON_METHODS` to
//! either a spine payload (read/low-risk against the backend) or a periphery
//! arm that returns `method_not_implemented` after passing the operation-token
//! policy check.
//!
//! The HTTP transport (loopback only) is intentionally hand-rolled and lives
//! here; production IPC will live in [`transport`] once wired.

use std::fs;
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, DEV_LOOPBACK_HTTP_ENDPOINT, DEV_LOOPBACK_RPC_PATH, DaemonRequest, DaemonState,
    DaemonStatus, MutationClassification, PRODUCT_VERSION, ResponseEnvelope, Source,
    known_methods::{Surface, find},
};
use serde_json::Value;

pub mod backend;
pub mod event_stream;
pub mod transport;

pub use backend::{DaemonBackend, DaemonRuntimeInfo, InMemoryBackend, MountRecord, TransferRecord};
pub use transport::{TransportDescriptor, TransportKind};

use backend as b;

pub const LOCAL_TOKEN_ENV: &str = "BIOHAZARDFS_LOCAL_TOKEN";
pub const STATE_PATH_ENV: &str = "BIOHAZARDFS_STATE_PATH";
pub const WORKSPACE_ROOT_ENV: &str = "BIOHAZARDFS_WORKSPACE_ROOT";
const MAX_RPC_BODY_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 64;

/// Configuration for the dev-loopback HTTP transport. The backend is shared
/// across connections so dispatch sees one consistent in-memory state.
#[derive(Debug, Clone)]
pub struct DevLoopbackConfig {
    pub addr: String,
    pub local_token: String,
    pub backend: Arc<DaemonBackend>,
}

impl DevLoopbackConfig {
    /// Build a config with a fresh seeded backend bound to `addr`.
    pub fn new(addr: impl Into<String>, local_token: impl Into<String>) -> Self {
        let addr = addr.into();
        let backend = match std::env::var_os(STATE_PATH_ENV) {
            Some(path) => match DaemonBackend::new_persistent(addr.clone(), path) {
                Ok(backend) => Arc::new(backend),
                Err(error) => {
                    eprintln!("failed to open daemon persistent state: {}", error.message);
                    std::process::exit(2);
                }
            },
            None => Arc::new(DaemonBackend::new(addr.clone())),
        };
        Self {
            addr,
            local_token: local_token.into(),
            backend,
        }
    }

    /// Build a config that shares an existing backend (used by tests that seed
    /// state before binding the listener).
    pub fn with_backend(
        addr: impl Into<String>,
        local_token: impl Into<String>,
        backend: Arc<DaemonBackend>,
    ) -> Self {
        Self {
            addr: addr.into(),
            local_token: local_token.into(),
            backend,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DaemonHttpClient {
    endpoint: String,
    local_token: String,
}

impl DaemonHttpClient {
    pub fn new(endpoint: impl Into<String>, local_token: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            local_token: local_token.into(),
        }
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn call_status(&self, source: Source) -> Result<DaemonStatus, DaemonClientError> {
        let request = DaemonRequest::new("daemon.status", source);
        let envelope = self.call::<DaemonStatus>(&request)?;
        if envelope.ok {
            envelope
                .data
                .ok_or(DaemonClientError::Protocol("missing daemon status data"))
        } else {
            Err(DaemonClientError::Daemon(envelope.error.unwrap_or_else(
                || ApiError::new("daemon_error", "daemon returned an error"),
            )))
        }
    }

    pub fn call<T>(&self, request: &DaemonRequest) -> Result<ResponseEnvelope<T>, DaemonClientError>
    where
        T: serde::de::DeserializeOwned,
    {
        validate_loopback_addr(&self.endpoint).map_err(DaemonClientError::InvalidEndpoint)?;

        if self.local_token.is_empty() {
            return Err(DaemonClientError::MissingToken);
        }

        let body = serde_json::to_string(request)?;
        let mut stream = connect_loopback(&self.endpoint)?;
        let http_request = format!(
            "POST {DEV_LOOPBACK_RPC_PATH} HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            self.endpoint,
            self.local_token,
            body.len(),
            body
        );

        stream.write_all(http_request.as_bytes())?;

        let mut response = String::new();
        stream.read_to_string(&mut response)?;
        let status_ok =
            response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200");
        let body = response
            .split("\r\n\r\n")
            .nth(1)
            .ok_or(DaemonClientError::Protocol("malformed HTTP response"))?;
        let envelope = serde_json::from_str::<ResponseEnvelope<T>>(body)?;

        if status_ok || !envelope.ok {
            Ok(envelope)
        } else {
            Err(DaemonClientError::Protocol("unexpected daemon HTTP status"))
        }
    }
}

#[derive(Debug)]
pub enum DaemonClientError {
    InvalidEndpoint(String),
    MissingToken,
    Io(std::io::Error),
    Json(serde_json::Error),
    Daemon(ApiError),
    Protocol(&'static str),
}

impl std::fmt::Display for DaemonClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint(message) => {
                write!(formatter, "invalid daemon endpoint: {message}")
            }
            Self::MissingToken => write!(formatter, "missing local daemon token"),
            Self::Io(error) => write!(formatter, "daemon I/O error: {error}"),
            Self::Json(error) => write!(formatter, "daemon JSON error: {error}"),
            Self::Daemon(error) => write!(formatter, "{}: {}", error.code, error.message),
            Self::Protocol(message) => write!(formatter, "daemon protocol error: {message}"),
        }
    }
}

impl std::error::Error for DaemonClientError {}

impl From<std::io::Error> for DaemonClientError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for DaemonClientError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

pub fn daemon_status(endpoint: impl Into<String>) -> DaemonStatus {
    DaemonStatus {
        name: "biohazardfsd".to_string(),
        version: PRODUCT_VERSION.to_string(),
        state: DaemonState::Ready,
        transport: "dev_loopback_http_json_rpc".to_string(),
        endpoint: endpoint.into(),
    }
}

/// Dispatch a daemon RPC against `backend`. The method must appear in
/// `known_methods::DAEMON_METHODS` (else `method_not_found`). Destructive /
/// admin / data-moving methods require a valid operation token under the
/// AgentSafe mutation policy; the token check runs uniformly across spine and
/// periphery methods, so a missing token is reported before
/// `method_not_implemented`.
pub fn dispatch_rpc(backend: &DaemonBackend, request: &DaemonRequest) -> ResponseEnvelope<Value> {
    let request_id = request.request_id();
    let method = request.method.clone();
    let source = request.meta.source.clone();
    let params = request.params.clone();

    let result = match find(Surface::Daemon, &method) {
        None => Err(ApiError::new(
            "method_not_found",
            format!("unknown daemon method {method}"),
        )),
        Some(descriptor) => {
            if let Err(error) = ensure_operation_token(
                backend,
                &method,
                &params,
                descriptor.classification,
                source.clone(),
            ) {
                Err(error)
            } else {
                // dispatch_method takes `source` by value (it records audit
                // events); clone here so the envelope still has it below.
                dispatch_method(backend, &method, &params, source.clone(), &request_id)
            }
        }
    };

    match result {
        Ok(payload) => ResponseEnvelope::ok_with_request_id(method, request_id, payload, source),
        Err(error) => ResponseEnvelope::error_with_request_id(method, request_id, error, source),
    }
}

/// The dispatch table. Spine methods run against the backend; periphery
/// methods return `method_not_implemented`. Both lists stay within
/// `DAEMON_METHODS`; the wildcard arm only catches truly unknown names.
fn dispatch_method(
    backend: &DaemonBackend,
    method: &str,
    params: &Value,
    source: Source,
    _request_id: &str,
) -> Result<Value, ApiError> {
    match method {
        // ----- daemon/runtime -----
        "daemon.status" => b::daemon_status_payload(backend),
        "daemon.health" => b::daemon_health_payload(backend),
        "daemon.version" => b::daemon_version_payload(),
        "daemon.methods" => b::daemon_methods_payload(backend),
        "daemon.events.subscribe" => crate::event_stream::subscribe_payload(backend, params),
        "daemon.shutdown" | "daemon.restart" | "daemon.logs" => Err(not_implemented(method)),

        // ----- workspace runtime -----
        "workspace.status" => workspace_status_payload(backend),
        "workspace.list" => workspace_list_payload(params),

        // ----- auth/session -----
        "auth.status" => b::auth_status_payload(),
        "auth.whoami" => b::auth_whoami_payload(),
        "auth.credentials_path" => b::auth_credentials_path_payload(),
        "auth.enroll" | "auth.login_token" | "auth.logout" | "auth.rotate_credentials" => {
            Err(not_implemented(method))
        }

        // ----- config -----
        "config.path" => b::config_path_payload(),
        "config.show" => b::config_show_payload(),
        "config.validate" => b::config_validate_payload(),
        "config.get" => b::config_get_payload(params),
        "config.set" | "config.migrate" => Err(not_implemented(method)),

        // ----- mount -----
        "mount.status" => b::mount_status_payload(backend),
        "mount.list" => b::mount_list_payload(backend),
        "mount.attach" | "mount.detach" | "mount.repair" => Err(not_implemented(method)),

        // ----- file -----
        "file.stat" => b::file_stat_payload(backend, params),
        "file.list" => b::file_list_payload(backend, params),
        "file.checksum" => b::file_checksum_payload(backend, params),
        "file.history" => b::file_history_payload(backend, params),
        "file.versions" => b::file_versions_payload(backend, params),
        "file.write" => b::file_write_payload(backend, params, source),
        "file.read" => b::file_read_payload(backend, params),
        "file.mkdir" => b::file_mkdir_payload(backend, params, source),
        "file.rename" => b::file_rename_payload(backend, params, source),
        "file.restore" | "file.delete" | "file.move" | "file.copy" => Err(not_implemented(method)),

        // ----- cache -----
        "cache.status" => b::cache_status_payload(backend),
        "cache.list" => b::cache_list_payload(backend),
        "cache.pin" => b::cache_pin_payload(backend, params, source),
        "cache.unpin" => b::cache_unpin_payload(backend, params, source),
        "cache.hydrate" => b::cache_hydrate_payload(backend, params, source),
        "cache.dehydrate" => b::cache_dehydrate_payload(backend, params, source),
        "cache.verify" => b::cache_verify_payload(backend),
        "cache.evict" | "cache.move" | "cache.repair" => Err(not_implemented(method)),

        // ----- transfer -----
        "transfer.list" => b::transfer_list_payload(backend),
        "transfer.status" => b::transfer_status_payload(backend, params),
        "transfer.pause" | "transfer.resume" | "transfer.cancel" | "transfer.retry" => {
            Err(not_implemented(method))
        }

        // ----- snapshot (list is spine; mutations are periphery) -----
        "snapshot.list" => b::snapshot_list_payload(backend),
        "snapshot.create" | "snapshot.mount" | "snapshot.unmount" | "snapshot.diff"
        | "snapshot.restore" => Err(not_implemented(method)),

        // ----- lock -----
        "lock.list" => b::lock_list_payload(backend),
        "lock.acquire" => b::lock_acquire_payload(backend, params, source),
        "lock.release" => b::lock_release_payload(backend, params, source),
        "lock.status" => b::lock_status_payload(backend, params),
        "lock.extend" => b::lock_extend_payload(backend, params, source),
        "lock.break" => Err(not_implemented(method)),

        // ----- conflict -----
        "conflict.list" => b::conflict_list_payload(backend),
        "conflict.show" => b::conflict_show_payload(backend, params),
        "conflict.resolve" | "conflict.preserve_all" => Err(not_implemented(method)),

        // ----- workset -----
        "workset.list" => b::workset_list_payload(backend),
        "workset.show" => b::workset_show_payload(params),
        "workset.activate" | "workset.deactivate" | "workset.sync" | "workset.create"
        | "workset.update" => Err(not_implemented(method)),

        // ----- collaboration/share (reads spine; mutations periphery) -----
        "invite.list" => b::invite_list_payload(),
        "share.list" => b::share_list_payload(),
        "grant.list" => b::grant_list_payload(),
        "publish.list" => b::publish_list_payload(),
        "invite.create" | "invite.revoke" | "share.create" | "share.revoke" | "grant.set"
        | "grant.revoke" | "publish.create" | "publish.revoke" => Err(not_implemented(method)),

        // ----- audit (reads spine; export periphery) -----
        "audit.events" => b::audit_events_payload(backend, params),
        "audit.event" => b::audit_event_payload(backend, params),
        "audit.actor" => b::audit_actor_payload(backend, params),
        "audit.export" => Err(not_implemented(method)),

        // ----- admin (all periphery until permission gating lands) -----
        "admin.user.list"
        | "admin.user.show"
        | "admin.device.list"
        | "admin.device.revoke"
        | "admin.token.revoke"
        | "admin.retention.show"
        | "admin.retention.set"
        | "admin.support_bundle.create" => Err(not_implemented(method)),

        // ----- schema (list/method spine; event/error/config/all periphery) -----
        "schema.list" => b::schema_list_payload(),
        "schema.method" => b::schema_method_payload(params),
        "schema.event" | "schema.error" | "schema.config" | "schema.all" => {
            Err(not_implemented(method))
        }

        // Anything else is not a registered daemon method.
        _ => Err(ApiError::new(
            "method_not_found",
            format!("unknown daemon method {method}"),
        )),
    }
}

/// Operation-token policy check. Under AgentSafe, only Read/LowRisk methods
/// proceed without a token; Destructive/Admin/DataMoving methods must present a
/// token whose method/classification/source and params hash all match. The
/// check runs before the dispatch table, so periphery destructive methods
/// surface the same policy error as spine ones would.
fn ensure_operation_token(
    backend: &DaemonBackend,
    method: &str,
    params: &Value,
    classification: MutationClassification,
    source: Source,
) -> Result<(), ApiError> {
    use MutationClassification::*;
    match classification {
        Read | LowRisk => Ok(()),
        Destructive | Admin | DataMoving => {
            match params.get("operation_token").and_then(Value::as_str) {
                Some(token) => {
                    backend.validate_operation_token(
                        token,
                        method,
                        classification,
                        source,
                        params,
                    )?;
                    Ok(())
                }
                None => Err(operation_token_required_error(method, classification)),
            }
        }
    }
}

fn operation_token_required_error(
    method: &str,
    classification: MutationClassification,
) -> ApiError {
    ApiError::with_details(
        "operation_token_required",
        format!("{method} requires a dry-run operation token under the current mutation policy."),
        serde_json::json!({
            "policy": "agent_safe",
            "classification": serde_json::to_value(classification).unwrap_or(Value::Null),
        }),
    )
}

fn not_implemented(method: &str) -> ApiError {
    ApiError::new(
        "method_not_implemented",
        format!("{method} is wired but not yet implemented in the daemon scaffold"),
    )
}

fn workspace_status_payload(backend: &DaemonBackend) -> Result<Value, ApiError> {
    let endpoint = backend.runtime().endpoint;
    let root = workspace_root_from_env();
    let root_display = root.as_ref().map(|path| path.to_string_lossy().to_string());
    let exists = root.as_ref().is_some_and(|path| path.is_dir());
    let writable = root.as_ref().is_some_and(|path| {
        fs::metadata(path)
            .map(|metadata| metadata.is_dir() && !metadata.permissions().readonly())
            .unwrap_or(false)
    });
    Ok(serde_json::json!({
        "state": if root.is_some() && exists && writable { "ready" } else { "unconfigured" },
        "transport": "dev_loopback_http_json_rpc",
        "endpoint": endpoint,
        "root_configured": root.is_some(),
        "root": root_display,
        "root_exists": exists,
        "root_writable": writable,
    }))
}

fn workspace_list_payload(params: &Value) -> Result<Value, ApiError> {
    let root = workspace_root_from_env().ok_or_else(|| {
        ApiError::new(
            "workspace_root_missing",
            format!("set {WORKSPACE_ROOT_ENV} to enable local workspace methods"),
        )
    })?;
    if !root.is_dir() {
        return Err(ApiError::new(
            "workspace_unavailable",
            "workspace root does not exist or is not a directory",
        ));
    }
    let relative = params
        .get("path")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let relative_path = validate_workspace_relative_path(relative)?;
    let canonical_root = fs::canonicalize(&root)
        .map_err(|_| ApiError::new("workspace_unavailable", "could not resolve workspace root"))?;
    let target = root.join(relative_path);
    let canonical_target = fs::canonicalize(&target)
        .map_err(|_| ApiError::new("workspace_path_not_found", "workspace path was not found"))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err(ApiError::new(
            "workspace_path_invalid",
            "workspace path must stay inside the workspace root",
        ));
    }
    let target_metadata = fs::metadata(&canonical_target)
        .map_err(|_| ApiError::new("workspace_path_not_found", "workspace path was not found"))?;
    if !target_metadata.is_dir() {
        return Err(ApiError::new(
            "workspace_path_not_directory",
            "workspace path is not a directory",
        ));
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(&canonical_target).map_err(|_| {
        ApiError::new(
            "workspace_unavailable",
            "could not list workspace directory",
        )
    })? {
        if entries.len() >= 500 {
            break;
        }
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        if name_text == ".VolumeIcon.icns"
            || name_text == ".DS_Store"
            || name_text.starts_with("._")
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let kind = if file_type.is_symlink() {
            "symlink"
        } else if file_type.is_dir() {
            "directory"
        } else if file_type.is_file() {
            "file"
        } else {
            "other"
        };
        entries.push(serde_json::json!({
            "name": name.to_string_lossy(),
            "kind": kind,
            "size_bytes": if file_type.is_file() { Some(metadata.len()) } else { None },
        }));
    }
    entries.sort_by(|left, right| {
        left.get("name")
            .and_then(|value| value.as_str())
            .cmp(&right.get("name").and_then(|value| value.as_str()))
    });
    Ok(serde_json::json!({
        "root": canonical_root.to_string_lossy(),
        "path": relative,
        "entries": entries,
    }))
}

fn workspace_root_from_env() -> Option<PathBuf> {
    std::env::var(WORKSPACE_ROOT_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn validate_workspace_relative_path(value: &str) -> Result<&Path, ApiError> {
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(ApiError::new(
            "workspace_path_invalid",
            "workspace path contains invalid characters",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(ApiError::new(
            "workspace_path_invalid",
            "workspace path must stay inside the workspace root",
        ));
    }
    Ok(path)
}

pub fn run_dev_loopback_http(config: DevLoopbackConfig) -> std::io::Result<()> {
    validate_loopback_addr(&config.addr)
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;
    if config.local_token.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "local token must not be empty",
        ));
    }

    let listener = TcpListener::bind(&config.addr)?;
    eprintln!(
        "biohazardfsd dev loopback JSON-RPC listening on http://{}{}",
        config.addr, DEV_LOOPBACK_RPC_PATH
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if let Err(error) = handle_stream(stream, &config) {
                    eprintln!("biohazardfsd request error: {error}");
                }
            }
            Err(error) => eprintln!("biohazardfsd accept error: {error}"),
        }
    }

    Ok(())
}

fn connect_loopback(endpoint: &str) -> Result<TcpStream, DaemonClientError> {
    let mut addrs = endpoint.to_socket_addrs().map_err(|error| {
        DaemonClientError::InvalidEndpoint(format!("could not resolve {endpoint}: {error}"))
    })?;
    let addr = addrs.next().ok_or_else(|| {
        DaemonClientError::InvalidEndpoint(format!("could not resolve {endpoint}: no address"))
    })?;

    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(700))?;
    stream.set_read_timeout(Some(Duration::from_millis(1200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(1200)))?;
    Ok(stream)
}

pub fn validate_loopback_addr(addr: &str) -> Result<(), String> {
    let parsed: SocketAddr = addr
        .parse()
        .map_err(|error| format!("{addr} is not a valid socket address: {error}"))?;
    match parsed.ip() {
        IpAddr::V4(ip) if ip.is_loopback() => Ok(()),
        IpAddr::V6(ip) if ip.is_loopback() => Ok(()),
        _ => Err("dev loopback HTTP may only bind/connect to 127.0.0.1 or [::1]".to_string()),
    }
}

fn handle_stream(mut stream: TcpStream, config: &DevLoopbackConfig) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(1200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(1200)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let request_line = read_limited_line(&mut reader, MAX_REQUEST_LINE_BYTES)?;

    let mut content_length = 0usize;
    let mut authorized = false;
    let mut header_bytes = 0usize;
    let mut saw_end_headers = false;

    for _ in 0..MAX_HEADERS {
        let header = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES)?;
        header_bytes += header.len();
        if header_bytes > MAX_HEADER_BYTES {
            let envelope: ResponseEnvelope<Value> = ResponseEnvelope::error(
                "daemon.request",
                ApiError::new(
                    "headers_too_large",
                    "daemon request headers exceed scaffold limit",
                ),
                Source::Server,
            );
            return write_json_response(
                &mut stream,
                "431 Request Header Fields Too Large",
                &envelope,
            );
        }

        let header_trimmed = header.trim_end();
        if header_trimmed.is_empty() {
            saw_end_headers = true;
            break;
        }

        if let Some((name, value)) = header_trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().unwrap_or_default();
            }
            if name.eq_ignore_ascii_case("authorization") {
                authorized = value.trim() == format!("Bearer {}", config.local_token);
            }
        }
    }

    if !saw_end_headers {
        let envelope: ResponseEnvelope<Value> = ResponseEnvelope::error(
            "daemon.request",
            ApiError::new("too_many_headers", "daemon request has too many headers"),
            Source::Server,
        );
        return write_json_response(
            &mut stream,
            "431 Request Header Fields Too Large",
            &envelope,
        );
    }

    let method = request_line.split_whitespace().next().unwrap_or_default();
    let path = request_line.split_whitespace().nth(1).unwrap_or_default();

    if content_length > MAX_RPC_BODY_BYTES {
        let envelope: ResponseEnvelope<Value> = ResponseEnvelope::error(
            "daemon.request",
            ApiError::new(
                "request_too_large",
                "daemon request body exceeds scaffold limit",
            ),
            Source::Server,
        );
        return write_json_response(&mut stream, "413 Payload Too Large", &envelope);
    }

    if method != "POST" || path != DEV_LOOPBACK_RPC_PATH {
        drain_body(&mut reader, content_length)?;
        let envelope: ResponseEnvelope<Value> = ResponseEnvelope::error(
            "daemon.request",
            ApiError::new(
                "invalid_transport_request",
                "daemon HTTP transport only accepts POST /rpc",
            ),
            Source::Server,
        );
        return write_json_response(&mut stream, "404 Not Found", &envelope);
    }

    if !authorized {
        drain_body(&mut reader, content_length)?;
        let envelope: ResponseEnvelope<Value> = ResponseEnvelope::error(
            "daemon.request",
            ApiError::new("unauthorized", "missing or invalid local daemon token"),
            Source::Server,
        );
        return write_json_response(&mut stream, "401 Unauthorized", &envelope);
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;

    let response = match serde_json::from_slice::<DaemonRequest>(&body) {
        Ok(request) => dispatch_rpc(&config.backend, &request),
        Err(error) => ResponseEnvelope::error(
            "daemon.request",
            ApiError::new(
                "invalid_request",
                format!("invalid daemon request envelope: {error}"),
            ),
            Source::Server,
        ),
    };

    write_json_response(&mut stream, "200 OK", &response)
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

fn drain_body(reader: &mut BufReader<TcpStream>, mut remaining: usize) -> std::io::Result<()> {
    let mut buffer = [0_u8; 1024];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len());
        reader.read_exact(&mut buffer[..chunk])?;
        remaining -= chunk;
    }
    Ok(())
}

fn write_json_response<T>(
    stream: &mut TcpStream,
    status: &str,
    envelope: &ResponseEnvelope<T>,
) -> std::io::Result<()>
where
    T: serde::Serialize,
{
    let body = serde_json::to_string(envelope).map_err(std::io::Error::other)?;
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )?;
    stream.flush()
}

pub fn default_dev_loopback_endpoint() -> &'static str {
    DEV_LOOPBACK_HTTP_ENDPOINT
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::DaemonRequestMeta;

    fn make_request(method: &str, params: Value) -> DaemonRequest {
        let mut request = DaemonRequest::new(method, Source::Test);
        request.params = params;
        request
    }

    fn request_with_id(id: &str, method: &str, params: Value) -> DaemonRequest {
        DaemonRequest {
            id: Some(id.to_string()),
            method: method.to_string(),
            params,
            meta: DaemonRequestMeta::new(Source::Test),
        }
    }

    #[test]
    fn rejects_non_loopback_dev_http_addresses() {
        assert!(validate_loopback_addr("127.0.0.1:47666").is_ok());
        assert!(validate_loopback_addr("[::1]:47666").is_ok());
        assert!(validate_loopback_addr("0.0.0.0:47666").is_err());
        assert!(validate_loopback_addr("192.168.1.128:47666").is_err());
    }

    #[test]
    fn dispatch_uses_json_rpc_method_shape_and_request_id() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = request_with_id("req_contract", "daemon.status", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(response.ok);
        assert_eq!(response.method, "daemon.status");
        assert_eq!(response.meta.request_id, "req_contract");
        assert_eq!(response.meta.source, Source::Test);
        assert_eq!(
            response.meta.schema_version,
            biohazardfs_api_types::DAEMON_SCHEMA_VERSION
        );
    }

    #[test]
    fn unknown_methods_return_error_envelope() {
        // cache.pin is now spine; use a genuinely unknown name to exercise the
        // method_not_found path.
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("cache.bogus_method", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(response.method, "cache.bogus_method");
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("method_not_found")
        );
    }

    #[test]
    fn daemon_methods_lists_every_registered_method() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("daemon.methods", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(response.ok);
        let methods = response.data.unwrap()["methods"]
            .as_array()
            .expect("methods is an array")
            .clone();
        let total = biohazardfs_api_types::known_methods::daemon_method_names().len();
        assert_eq!(methods.len(), total);
        // Sorted, deduped per the registry. serde_json::Value is not Ord, so
        // compare by the string method name rather than the Value itself.
        let mut sorted_names: Vec<String> = methods
            .iter()
            .map(|value| value.as_str().expect("method name is a string").to_string())
            .collect();
        sorted_names.sort();
        let original_names: Vec<String> = methods
            .iter()
            .map(|value| value.as_str().expect("method name is a string").to_string())
            .collect();
        assert_eq!(sorted_names, original_names);
    }

    #[test]
    fn file_stat_returns_seeded_node() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("file.stat", serde_json::json!({"node_id": "node_root"}));
        let response = dispatch_rpc(&backend, &request);
        assert!(response.ok);
        assert_eq!(response.data.unwrap()["kind"], "directory");
    }

    #[test]
    fn cache_pin_round_trips_through_dispatch() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("cache.pin", serde_json::json!({"node_id": "node_readme"}));
        let response = dispatch_rpc(&backend, &request);
        assert!(response.ok);
        let data = response.data.unwrap();
        assert_eq!(data["state"], "pinned");
        assert_eq!(data["pinned"], true);
    }

    #[test]
    fn periphery_methods_return_method_not_implemented() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        // daemon.shutdown is Admin -> needs a token first.
        let request = make_request("daemon.shutdown", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_required")
        );

        // file.move is DataMoving -> token required before periphery reply.
        let request = make_request("file.move", serde_json::json!({"node_id": "node_root"}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_required")
        );

        // auth.logout is Admin -> token required.
        let request = make_request("auth.logout", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_required")
        );
    }

    #[test]
    fn low_risk_periphery_method_returns_method_not_implemented_directly() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        // auth.enroll is LowRisk -> no token needed, falls straight to periphery.
        let request = make_request("auth.enroll", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("method_not_implemented")
        );
        // admin.* are all Admin -> token required (not method_not_implemented).
        let request = make_request("admin.user.list", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_required")
        );
    }

    #[test]
    fn destructive_method_runs_periphery_arm_with_valid_token() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let params = serde_json::json!({"node_id": "node_readme"});
        // Issue with the same source dispatch will use (Test) so the binding
        // check passes and the periphery arm runs.
        let token = backend.issue_operation_token(
            "file.delete",
            &params,
            MutationClassification::Destructive,
            Source::Test,
        );
        let mut with_token = params.clone();
        with_token["operation_token"] = Value::String(token.operation_token.clone());
        let request = make_request("file.delete", with_token);
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        // Token valid -> the periphery arm reports method_not_implemented.
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("method_not_implemented")
        );
    }

    #[test]
    fn destructive_method_rejects_drifted_token() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let params = serde_json::json!({"node_id": "node_readme"});
        // Issue with the dispatch source so the only divergence is the params.
        let token = backend.issue_operation_token(
            "file.delete",
            &params,
            MutationClassification::Destructive,
            Source::Test,
        );
        let drifted = serde_json::json!({
            "node_id": "node_root",
            "operation_token": token.operation_token,
        });
        let request = make_request("file.delete", drifted);
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_params_mismatch")
        );
        // Details carry expected/actual hashes for diagnostics.
        let details = response.error.unwrap().details.unwrap();
        assert!(details.get("expected").is_some());
        assert!(details.get("actual").is_some());
    }

    #[test]
    fn destructive_method_rejects_token_issued_for_different_method() {
        // Token binding: a token issued for cache.evict (destructive) must not
        // authorize file.delete even if params/source/classification line up.
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let params = serde_json::json!({"node_id": "node_readme"});
        let token = backend.issue_operation_token(
            "cache.evict",
            &params,
            MutationClassification::Destructive,
            Source::Test,
        );
        let mut with_token = params.clone();
        with_token["operation_token"] = Value::String(token.operation_token.clone());
        let request = make_request("file.delete", with_token);
        let response = dispatch_rpc(&backend, &request);
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|e| e.code.as_str()),
            Some("operation_token_mismatch")
        );
        let details = response.error.unwrap().details.unwrap();
        assert_eq!(details["field"], "method");
        assert_eq!(details["expected"], "cache.evict");
        assert_eq!(details["actual"], "file.delete");
    }

    #[test]
    fn operation_token_required_carries_policy_and_classification() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("mount.detach", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        let details = response.error.unwrap().details.unwrap();
        assert_eq!(details["policy"], "agent_safe");
        assert_eq!(details["classification"], "destructive");
    }

    #[test]
    fn events_subscribe_returns_ack_with_replay() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        let request = make_request("daemon.events.subscribe", serde_json::json!({}));
        let response = dispatch_rpc(&backend, &request);
        assert!(response.ok);
        let data = response.data.unwrap();
        assert_eq!(data["state"], "acknowledged");
        let replay = data["replay"].as_array().unwrap();
        assert!(!replay.is_empty());
        assert_eq!(
            data["schema_version"],
            biohazardfs_api_types::EVENT_SCHEMA_VERSION
        );
    }

    #[test]
    fn dev_http_rejects_missing_local_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener has address");
        let config = DevLoopbackConfig::new(addr.to_string(), "local_test_token");

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept test request");
            handle_stream(stream, &config).expect("handle test request");
        });

        let mut stream = TcpStream::connect(addr).expect("connect to test daemon");
        let body = serde_json::to_string(&DaemonRequest::new("daemon.status", Source::Test))
            .expect("request serializes");
        write!(
            stream,
            "POST {DEV_LOOPBACK_RPC_PATH} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("write test request");

        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read test response");
        assert!(response.starts_with("HTTP/1.1 401"), "{response}");
        assert!(response.contains("unauthorized"), "{response}");
        handle.join().expect("handler thread exits cleanly");
    }

    #[test]
    fn dev_http_client_uses_authenticated_json_rpc() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener has address");
        let config = DevLoopbackConfig::new(addr.to_string(), "local_test_token");

        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept test request");
            handle_stream(stream, &config).expect("handle test request");
        });

        let status = DaemonHttpClient::new(addr.to_string(), "local_test_token")
            .call_status(Source::Test)
            .expect("client status succeeds");
        assert_eq!(status.state, DaemonState::Ready);
        assert_eq!(status.transport, "dev_loopback_http_json_rpc");
        handle.join().expect("handler thread exits cleanly");
    }

    #[test]
    fn dev_loopback_config_shares_backend() {
        // Constructing a config twice yields independent backends; a single
        // backend shared across rebinds keeps state consistent.
        let backend = Arc::new(DaemonBackend::new("127.0.0.1:47666"));
        let config =
            DevLoopbackConfig::with_backend("127.0.0.1:47666", "tok", Arc::clone(&backend));
        assert!(Arc::ptr_eq(&config.backend, &backend));
    }
}
