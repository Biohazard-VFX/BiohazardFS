use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, ContentObjectGetResponse, ContentObjectPutResponse, NamespaceChildrenResponse,
    NamespaceNodeSummary, PRODUCT_VERSION, SERVER_SCHEMA_VERSION, ServerHealth, ServerHealthCheck,
    ServerResponseEnvelope, ServerState, ServerStatus, ServerVersion, Source,
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
    scopes_json: String,
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
    if method != "GET" && !(method == "PUT" && route_path == "/api/v1/objects/content") {
        return json_response(
            405,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new(
                    "method_not_allowed",
                    "server endpoint does not support this method",
                ),
                Source::Server,
            ),
        );
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
        "/api/v1/namespace/children" if method == "GET" => {
            namespace_children_response(query, headers, config)
        }
        "/api/v1/objects/content" if method == "PUT" => {
            content_object_put_response(headers, body, config)
        }
        "/api/v1/objects/content" if method == "GET" => {
            content_object_get_response(query, headers, config)
        }
        "/api/v1/namespace/children" | "/api/v1/objects/content" => json_response(
            405,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new(
                    "method_not_allowed",
                    "server endpoint does not support this method",
                ),
                Source::Server,
            ),
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

    object_store_check_payload_with_config(config)
        .map_err(|error| object_store_api_error(error, "object-store bucket is unavailable"))?;

    let content_hash = sha256_hex(body);
    let object_key = content_object_key(&subject.org_id, &content_hash);
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
        &["object:read", "object:*", "file:read", "server:read"],
    )
}

fn scopes_allow_object_write(scopes_json: &str) -> bool {
    scopes_allow_any(
        scopes_json,
        &["object:write", "object:*", "file:write", "server:write"],
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

    if method != "GET" && method != "PUT" {
        let (_status_code, body) = json_response(
            405,
            &ServerResponseEnvelope::<serde_json::Value>::error(
                "server.request",
                ApiError::new(
                    "method_not_allowed",
                    "server accepts GET and bounded PUT requests",
                ),
                Source::Server,
            ),
        );
        return write_http_response(&mut stream, 405, &body);
    }

    let (route_path, _) = split_path_and_query(path);
    let should_read_body = method == "PUT" && route_path == "/api/v1/objects/content";
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
}
