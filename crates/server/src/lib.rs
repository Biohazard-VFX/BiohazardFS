use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, PRODUCT_VERSION, SERVER_SCHEMA_VERSION, ServerHealth, ServerHealthCheck,
    ServerResponseEnvelope, ServerState, ServerStatus, ServerVersion, Source,
};

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:8080";
pub const CONTAINER_BIND_ADDR: &str = "0.0.0.0:8080";
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_CONCURRENT_CONNECTIONS: usize = 64;

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
                message: "database check is scaffolded".to_string(),
            },
            ServerHealthCheck {
                name: "object_store".to_string(),
                ok: true,
                message: "object-store check is scaffolded".to_string(),
            },
        ],
    }
}

pub fn migrate_payload() -> serde_json::Value {
    serde_json::json!({
        "name": "biohazardfs-server",
        "mode": "migrate",
        "status": "scaffold_noop",
        "applied_migrations": []
    })
}

pub fn worker_payload() -> serde_json::Value {
    serde_json::json!({
        "name": "biohazardfs-server",
        "mode": "worker",
        "status": "scaffold_ready",
        "queues": []
    })
}

pub fn dispatch_http_path(path: &str) -> (u16, String) {
    match path {
        "/healthz" | "/health" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.health", server_health(), Source::Server),
        ),
        "/readyz" | "/ready" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.ready", server_health(), Source::Server),
        ),
        "/version" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.version", server_version(), Source::Server),
        ),
        "/api/v1/status" => json_response(
            200,
            &ServerResponseEnvelope::ok("server.status", server_status("serve"), Source::Server),
        ),
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

pub fn serve(addr: &str) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    let active_connections = Arc::new(AtomicUsize::new(0));
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
                let spawn_result = std::thread::Builder::new()
                    .name("biohazardfs-server-http".to_string())
                    .spawn(move || {
                        if let Err(error) = handle_stream(stream) {
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

fn handle_stream(mut stream: TcpStream) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(1200)))?;
    stream.set_write_timeout(Some(Duration::from_millis(1200)))?;

    let mut reader = BufReader::new(stream.try_clone()?);
    let request_line = read_limited_line(&mut reader, MAX_REQUEST_LINE_BYTES)?;
    let mut saw_end_headers = false;

    for _ in 0..MAX_HEADERS {
        let header = read_limited_line(&mut reader, MAX_HEADER_LINE_BYTES)?;
        if header.trim_end().is_empty() {
            saw_end_headers = true;
            break;
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

    if method != "GET" {
        let (_status_code, body) = json_response(
            405,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new("method_not_allowed", "server scaffold only accepts GET"),
                Source::Server,
            ),
        );
        return write_http_response(&mut stream, 405, &body);
    }

    let (status_code, body) = dispatch_http_path(path);
    write_http_response(&mut stream, status_code, &body)
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
        404 => "Not Found",
        405 => "Method Not Allowed",
        431 => "Request Header Fields Too Large",
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
    fn dispatch_unknown_path_returns_not_found_envelope() {
        let (status_code, body) = dispatch_http_path("/missing");
        assert_eq!(status_code, 404);
        let value: serde_json::Value = serde_json::from_str(&body).expect("valid json");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "not_found");
    }
}
