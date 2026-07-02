use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, PRODUCT_VERSION, SERVER_SCHEMA_VERSION, ServerHealth, ServerHealthCheck,
    ServerResponseEnvelope, ServerState, ServerStatus, ServerVersion, Source,
};
use postgres::config::SslMode;
use postgres::{Client, Config, NoTls};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BIND_ADDR: &str = biohazardfs_core::config::DEFAULT_SERVER_BIND;
pub const CONTAINER_BIND_ADDR: &str = "0.0.0.0:8080";
const MAX_REQUEST_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;
const MAX_CONCURRENT_CONNECTIONS: usize = 64;
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
    let config = biohazardfs_core::config::RuntimeConfig::from_env();
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
    let config = biohazardfs_core::config::RuntimeConfig::from_env();
    let database_check = if config.database.url_set {
        match verify_latest_migration_from_env() {
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
    run_migrations_from_env()
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
        "/readyz" | "/ready" => {
            let readiness = server_readiness();
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

pub fn run_migrations_from_env() -> Result<MigrationReport, MigrationError> {
    let database_url = database_url_from_env()?;
    run_migrations(&database_url)
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

fn verify_latest_migration_from_env() -> Result<(), MigrationError> {
    let database_url = database_url_from_env()?;
    verify_latest_migration(&database_url)
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

fn database_url_from_env() -> Result<String, MigrationError> {
    let database_url = std::env::var(biohazardfs_core::config::ENV_DATABASE_URL).ok();
    validate_database_url(database_url.as_deref()).map(ToOwned::to_owned)
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
    &[Migration {
        version: "001",
        name: "baseline",
        sql: include_str!("../migrations/001_baseline.sql"),
    }]
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
}
