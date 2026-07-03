use std::fs;
use std::io::{BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, DEV_LOOPBACK_HTTP_ENDPOINT, DEV_LOOPBACK_RPC_PATH, DaemonRequest, DaemonState,
    DaemonStatus, PRODUCT_VERSION, ResponseEnvelope, Source,
};

pub const LOCAL_TOKEN_ENV: &str = "BIOHAZARDFS_LOCAL_TOKEN";
pub const WORKSPACE_ROOT_ENV: &str = "BIOHAZARDFS_WORKSPACE_ROOT";
const MAX_RPC_BODY_BYTES: usize = 1024 * 1024;
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_HEADERS: usize = 64;

#[derive(Debug, Clone)]
pub struct DevLoopbackConfig {
    pub addr: String,
    pub local_token: String,
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

pub fn dispatch_rpc(
    request: DaemonRequest,
    endpoint: impl Into<String>,
) -> ResponseEnvelope<serde_json::Value> {
    let request_id = request.request_id();
    let method = request.method.clone();
    let source = request.meta.source.clone();

    match method.as_str() {
        "daemon.status" | "daemon.health" => ResponseEnvelope::ok_with_request_id(
            method,
            request_id,
            serde_json::to_value(daemon_status(endpoint)).expect("daemon status serializes"),
            source,
        ),
        "daemon.methods" => ResponseEnvelope::ok_with_request_id(
            method,
            request_id,
            serde_json::json!({
                "methods": [
                    "daemon.status",
                    "daemon.health",
                    "daemon.methods",
                    "workspace.status",
                    "workspace.list"
                ],
                "transport": "dev_loopback_http_json_rpc"
            }),
            source,
        ),
        "workspace.status" => ResponseEnvelope::ok_with_request_id(
            method,
            request_id,
            workspace_status_payload(endpoint),
            source,
        ),
        "workspace.list" => match workspace_list_payload(&request.params) {
            Ok(payload) => {
                ResponseEnvelope::ok_with_request_id(method, request_id, payload, source)
            }
            Err(error) => {
                ResponseEnvelope::error_with_request_id(method, request_id, error, source)
            }
        },
        _ => ResponseEnvelope::error_with_request_id(
            method,
            request_id,
            ApiError::new("method_not_found", "unknown daemon method"),
            source,
        ),
    }
}

fn workspace_status_payload(endpoint: impl Into<String>) -> serde_json::Value {
    let root = workspace_root_from_env();
    let root_display = root.as_ref().map(|path| path.to_string_lossy().to_string());
    let exists = root.as_ref().is_some_and(|path| path.is_dir());
    let writable = root.as_ref().is_some_and(|path| {
        fs::metadata(path)
            .map(|metadata| metadata.is_dir() && !metadata.permissions().readonly())
            .unwrap_or(false)
    });
    serde_json::json!({
        "state": if root.is_some() && exists && writable { "ready" } else { "unconfigured" },
        "transport": "dev_loopback_http_json_rpc",
        "endpoint": endpoint.into(),
        "root_configured": root.is_some(),
        "root": root_display,
        "root_exists": exists,
        "root_writable": writable,
    })
}

fn workspace_list_payload(params: &serde_json::Value) -> Result<serde_json::Value, ApiError> {
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
        let entry = entry.map_err(|_| {
            ApiError::new(
                "workspace_unavailable",
                "could not read workspace directory entry",
            )
        })?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
            ApiError::new("workspace_unavailable", "could not inspect workspace entry")
        })?;
        let file_type = entry.file_type().map_err(|_| {
            ApiError::new("workspace_unavailable", "could not inspect workspace entry")
        })?;
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
            "name": entry.file_name().to_string_lossy(),
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
            let envelope: ResponseEnvelope<serde_json::Value> = ResponseEnvelope::error(
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
        let envelope: ResponseEnvelope<serde_json::Value> = ResponseEnvelope::error(
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
        let envelope: ResponseEnvelope<serde_json::Value> = ResponseEnvelope::error(
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
        let envelope: ResponseEnvelope<serde_json::Value> = ResponseEnvelope::error(
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
        let envelope: ResponseEnvelope<serde_json::Value> = ResponseEnvelope::error(
            "daemon.request",
            ApiError::new("unauthorized", "missing or invalid local daemon token"),
            Source::Server,
        );
        return write_json_response(&mut stream, "401 Unauthorized", &envelope);
    }

    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;

    let response = match serde_json::from_slice::<DaemonRequest>(&body) {
        Ok(request) => dispatch_rpc(request, config.addr.clone()),
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

    #[test]
    fn rejects_non_loopback_dev_http_addresses() {
        assert!(validate_loopback_addr("127.0.0.1:47666").is_ok());
        assert!(validate_loopback_addr("[::1]:47666").is_ok());
        assert!(validate_loopback_addr("0.0.0.0:47666").is_err());
        assert!(validate_loopback_addr("192.168.1.128:47666").is_err());
    }

    #[test]
    fn dispatch_uses_json_rpc_method_shape_and_request_id() {
        let request = DaemonRequest {
            id: Some("req_contract".to_string()),
            method: "daemon.status".to_string(),
            params: serde_json::json!({}),
            meta: biohazardfs_api_types::DaemonRequestMeta::new(Source::Test),
        };

        let response = dispatch_rpc(request, "127.0.0.1:47666");
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
        let request = DaemonRequest::new("cache.pin", Source::Test);
        let response = dispatch_rpc(request, "127.0.0.1:47666");
        assert!(!response.ok);
        assert_eq!(response.method, "cache.pin");
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("method_not_found")
        );
    }

    #[test]
    fn dev_http_rejects_missing_local_token() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
        let addr = listener.local_addr().expect("listener has address");
        let config = DevLoopbackConfig {
            addr: addr.to_string(),
            local_token: "local_test_token".to_string(),
        };

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
        let config = DevLoopbackConfig {
            addr: addr.to_string(),
            local_token: "local_test_token".to_string(),
        };

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
}
