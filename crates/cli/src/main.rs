use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use biohazardfs_api_types::{
    ApiError, ClientStatus, CommandResponseEnvelope, CommandSchemaSummary,
    DEV_LOOPBACK_HTTP_ENDPOINT, DaemonRequest, DaemonStatus, Source,
};
use biohazardfs_core::config::{
    CONFIG_SCHEMA_VERSION, ConfigError, ConfigLoadOptions, ENV_PROFILE, LoadedConfig,
    RuntimeConfig, resolve_config_file_path,
};
use biohazardfs_daemon::{DaemonClientError, DaemonHttpClient, LOCAL_TOKEN_ENV};
use clap::{Parser, Subcommand};

const EXIT_OK: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_AUTH: u8 = 3;
const EXIT_DAEMON_UNAVAILABLE: u8 = 6;
const EXIT_SERVER_UNAVAILABLE: u8 = 6;
const SERVER_TOKEN_ENV: &str = "BIOHAZARDFS_SERVER_TOKEN";
const MAX_NAMESPACE_LIMIT: u32 = 500;

#[derive(Debug, Parser)]
#[command(name = "biohazardfs")]
#[command(about = "BiohazardFS virtual sync client")]
struct Cli {
    /// Development/test loopback HTTP daemon endpoint. Production will use descriptor-discovered IPC.
    #[arg(long, global = true, default_value = DEV_LOOPBACK_HTTP_ENDPOINT)]
    daemon_endpoint: String,

    /// Explicit TOML config file path. This is safe for argv; secrets are not.
    #[arg(long = "config", global = true, value_name = "PATH")]
    config_file: Option<PathBuf>,

    /// Config profile name. Overrides env/config-file profile selection.
    #[arg(long, global = true, value_name = "NAME")]
    profile: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show client and daemon reachability status.
    Status,
    /// Daemon-related commands.
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    /// Config inspection and validation.
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Server-backed namespace metadata commands.
    Namespace {
        #[command(subcommand)]
        command: NamespaceCommand,
    },
    /// Schema-introspection scaffold.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Backward-compatible command schema scaffold.
    Commands,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    /// Show daemon status by calling the daemon method registry.
    Status,
    /// List daemon RPC methods exposed by the scaffold daemon.
    Methods,
}

#[derive(Debug, Subcommand)]
enum NamespaceCommand {
    /// List live child nodes visible to the authenticated server token.
    Children {
        /// Optional parent node ID. Omit for root children.
        #[arg(long)]
        parent: Option<String>,
        /// Maximum number of children to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    /// Print the resolved config file path without parsing the file.
    Path,
    /// Print the resolved config. Output is redacted even without --redacted.
    Show {
        /// Explicitly request redacted output. Secrets are never printed by this scaffold.
        #[arg(long)]
        redacted: bool,
    },
    /// Parse and validate config, returning warnings in the command envelope.
    Validate,
}

#[derive(Debug, Subcommand)]
enum SchemaCommand {
    /// Summarize an implemented command schema.
    Command { name: String },
    /// List known scaffold commands.
    List,
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let command = cli.command.take().unwrap_or(Command::Status);
    let (output, code) = match command {
        Command::Status => client_status_json(&cli),
        Command::Daemon {
            command: DaemonCommand::Status,
        } => daemon_status_json(&cli),
        Command::Daemon {
            command: DaemonCommand::Methods,
        } => daemon_methods_json(&cli),
        Command::Config { command } => config_json(&cli, command),
        Command::Namespace { command } => namespace_json(&cli, command),
        Command::Schema { command } => (schema_json(command), EXIT_OK),
        Command::Commands => (schema_json(SchemaCommand::List), EXIT_OK),
    };

    println!("{output}");
    ExitCode::from(code)
}

fn client_status_json(cli: &Cli) -> (String, u8) {
    let daemon_reachable = local_token().is_some_and(|token| {
        DaemonHttpClient::new(&cli.daemon_endpoint, token)
            .call_status(Source::Cli)
            .is_ok()
    });
    let envelope = CommandResponseEnvelope::ok(
        "client.status",
        ClientStatus {
            name: "biohazardfs".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            daemon_transport: "dev_loopback_http_json_rpc".to_string(),
            daemon_endpoint: Some(cli.daemon_endpoint.clone()),
            daemon_reachable,
        },
        Source::Cli,
    );
    (
        serde_json::to_string_pretty(&envelope).expect("client status serializes"),
        EXIT_OK,
    )
}

fn daemon_status_json(cli: &Cli) -> (String, u8) {
    let Some(token) = local_token() else {
        let envelope: CommandResponseEnvelope<DaemonStatus> = CommandResponseEnvelope::error(
            "daemon.status",
            ApiError::new(
                "auth_required",
                format!("set {LOCAL_TOKEN_ENV} to call the local daemon"),
            ),
            Source::Cli,
        );
        return (
            serde_json::to_string_pretty(&envelope).expect("daemon error serializes"),
            EXIT_AUTH,
        );
    };

    let client = DaemonHttpClient::new(&cli.daemon_endpoint, token);
    match client.call_status(Source::Cli) {
        Ok(status) => {
            let envelope = CommandResponseEnvelope::ok("daemon.status", status, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("daemon status serializes"),
                EXIT_OK,
            )
        }
        Err(error) => {
            let code = daemon_error_code(&error);
            let envelope: CommandResponseEnvelope<DaemonStatus> = CommandResponseEnvelope::error(
                "daemon.status",
                ApiError::new(code, error.to_string()),
                Source::Cli,
            );
            (
                serde_json::to_string_pretty(&envelope).expect("daemon error serializes"),
                daemon_exit_code(code),
            )
        }
    }
}

fn daemon_methods_json(cli: &Cli) -> (String, u8) {
    let Some(token) = local_token() else {
        let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
            "daemon.methods",
            ApiError::new(
                "auth_required",
                format!("set {LOCAL_TOKEN_ENV} to call the local daemon"),
            ),
            Source::Cli,
        );
        return (
            serde_json::to_string_pretty(&envelope).expect("daemon error serializes"),
            EXIT_AUTH,
        );
    };

    let client = DaemonHttpClient::new(&cli.daemon_endpoint, token);
    let request = DaemonRequest::new("daemon.methods", Source::Cli);
    match client.call::<serde_json::Value>(&request) {
        Ok(envelope) if envelope.ok => {
            let output = CommandResponseEnvelope::ok(
                "daemon.methods",
                envelope.data.unwrap_or_else(|| serde_json::json!({})),
                Source::Cli,
            );
            (
                serde_json::to_string_pretty(&output).expect("daemon methods serializes"),
                EXIT_OK,
            )
        }
        Ok(envelope) => {
            let error = envelope
                .error
                .unwrap_or_else(|| ApiError::new("daemon_error", "daemon returned an error"));
            let normalized_error = if error.code == "unauthorized" {
                ApiError::new("auth_required", "daemon rejected the local auth token")
            } else {
                error
            };
            let exit_code = if normalized_error.code == "auth_required" {
                EXIT_AUTH
            } else {
                EXIT_DAEMON_UNAVAILABLE
            };
            let output: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("daemon.methods", normalized_error, Source::Cli);
            (
                serde_json::to_string_pretty(&output).expect("daemon error serializes"),
                exit_code,
            )
        }
        Err(error) => {
            let code = daemon_error_code(&error);
            let output: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
                "daemon.methods",
                ApiError::new(code, error.to_string()),
                Source::Cli,
            );
            (
                serde_json::to_string_pretty(&output).expect("daemon error serializes"),
                daemon_exit_code(code),
            )
        }
    }
}

fn namespace_json(cli: &Cli, command: NamespaceCommand) -> (String, u8) {
    match command {
        NamespaceCommand::Children { parent, limit } => namespace_children_json(cli, parent, limit),
    }
}

fn namespace_children_json(cli: &Cli, parent: Option<String>, limit: u32) -> (String, u8) {
    let Some(token) = server_token() else {
        let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
            "namespace.children",
            ApiError::new(
                "auth_required",
                format!("set {SERVER_TOKEN_ENV} to call BiohazardFS server namespace APIs"),
            ),
            Source::Cli,
        );
        return (
            serde_json::to_string_pretty(&envelope).expect("namespace auth error serializes"),
            EXIT_AUTH,
        );
    };

    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json("namespace.children", error),
    };

    if let Err(error) = validate_namespace_limit(limit) {
        let envelope: CommandResponseEnvelope<serde_json::Value> =
            CommandResponseEnvelope::error("namespace.children", error, Source::Cli);
        return (
            serde_json::to_string_pretty(&envelope).expect("limit validation error serializes"),
            EXIT_USAGE,
        );
    }

    let mut path = format!("/api/v1/namespace/children?limit={limit}");
    if let Some(parent) = parent.as_deref() {
        let parent = match validate_node_id_query_value(parent) {
            Ok(parent) => parent,
            Err(error) => {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error("namespace.children", error, Source::Cli);
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("namespace validation error serializes"),
                    EXIT_USAGE,
                );
            }
        };
        path.push_str("&parent=");
        path.push_str(parent);
    }

    match server_get_json(&loaded.config.server.public_url, &path, Some(&token)) {
        Ok((_status, payload)) if payload.get("ok").and_then(|ok| ok.as_bool()) == Some(true) => {
            let data = payload
                .get("data")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let envelope = CommandResponseEnvelope::ok("namespace.children", data, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("namespace children serializes"),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => {
            let error = payload
                .get("error")
                .cloned()
                .and_then(|error| serde_json::from_value::<ApiError>(error).ok())
                .unwrap_or_else(|| ApiError::new("server_error", "server returned an error"));
            let exit_code = if status == 401
                || status == 403
                || matches!(error.code.as_str(), "auth_required" | "auth_scope_missing")
            {
                EXIT_AUTH
            } else if status == 400 || error.code == "invalid_limit" {
                EXIT_USAGE
            } else {
                EXIT_SERVER_UNAVAILABLE
            };
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("namespace.children", error, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("namespace error serializes"),
                exit_code,
            )
        }
        Err(error) => {
            let exit_code = if matches!(error.code, "invalid_server_url" | "insecure_server_url") {
                EXIT_USAGE
            } else {
                EXIT_SERVER_UNAVAILABLE
            };
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error(
                    "namespace.children",
                    ApiError::new(error.code, error.message),
                    Source::Cli,
                );
            (
                serde_json::to_string_pretty(&envelope).expect("namespace client error serializes"),
                exit_code,
            )
        }
    }
}

fn config_json(cli: &Cli, command: ConfigCommand) -> (String, u8) {
    match command {
        ConfigCommand::Path => config_path_json(cli),
        ConfigCommand::Show { redacted } => config_show_json(cli, redacted),
        ConfigCommand::Validate => config_validate_json(cli),
    }
}

fn config_path_json(cli: &Cli) -> (String, u8) {
    let options = config_load_options(cli);
    let path = resolve_config_file_path(&options);
    let profile = cli
        .profile
        .clone()
        .or_else(|| {
            std::env::var(ENV_PROFILE)
                .ok()
                .filter(|value| !value.is_empty())
        })
        .unwrap_or_else(|| biohazardfs_core::config::DEFAULT_PROFILE.to_string());
    let envelope = CommandResponseEnvelope::ok(
        "config.path",
        serde_json::json!({
            "path": path.to_string_lossy(),
            "exists": path.exists(),
            "profile": profile,
            "schema_version": CONFIG_SCHEMA_VERSION,
        }),
        Source::Cli,
    );
    (
        serde_json::to_string_pretty(&envelope).expect("config path serializes"),
        EXIT_OK,
    )
}

fn config_show_json(cli: &Cli, redacted: bool) -> (String, u8) {
    match load_config(cli) {
        Ok(loaded) => {
            let mut warnings = loaded.validation_warnings();
            if !redacted {
                warnings.push(biohazardfs_core::config::ConfigWarning {
                    code: "config_show_redacted_by_default".to_string(),
                    message: "config show output is redacted by default; pass --redacted to acknowledge this behavior".to_string(),
                });
            }
            config_ok_json("config.show", loaded, warnings)
        }
        Err(error) => config_error_json("config.show", error),
    }
}

fn config_validate_json(cli: &Cli) -> (String, u8) {
    match load_config(cli) {
        Ok(loaded) => {
            let warnings = loaded.validation_warnings();
            let data = serde_json::json!({
                "valid": true,
                "config_file_path": loaded.config_file_path,
                "config_file_exists": loaded.config_file_exists,
                "selected_profile": loaded.selected_profile,
                "schema_version": CONFIG_SCHEMA_VERSION,
                "warning_count": warnings.len(),
            });
            let mut envelope = CommandResponseEnvelope::ok("config.validate", data, Source::Cli);
            envelope.warnings = warnings
                .into_iter()
                .map(|warning| biohazardfs_api_types::Warning {
                    code: warning.code,
                    message: warning.message,
                })
                .collect();
            (
                serde_json::to_string_pretty(&envelope).expect("config validation serializes"),
                EXIT_OK,
            )
        }
        Err(error) => config_error_json("config.validate", error),
    }
}

fn config_ok_json(
    command: &str,
    loaded: LoadedConfig,
    warnings: Vec<biohazardfs_core::config::ConfigWarning>,
) -> (String, u8) {
    let mut envelope = CommandResponseEnvelope::ok(command, loaded, Source::Cli);
    envelope.warnings = warnings
        .into_iter()
        .map(|warning| biohazardfs_api_types::Warning {
            code: warning.code,
            message: warning.message,
        })
        .collect();
    (
        serde_json::to_string_pretty(&envelope).expect("config serializes"),
        EXIT_OK,
    )
}

fn config_error_json(command: &str, error: ConfigError) -> (String, u8) {
    let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
        command,
        ApiError::new(error.code, error.message),
        Source::Cli,
    );
    (
        serde_json::to_string_pretty(&envelope).expect("config error serializes"),
        EXIT_USAGE,
    )
}

fn load_config(cli: &Cli) -> Result<LoadedConfig, ConfigError> {
    RuntimeConfig::load(config_load_options(cli))
}

fn config_load_options(cli: &Cli) -> ConfigLoadOptions {
    ConfigLoadOptions {
        config_file: cli.config_file.clone(),
        profile: cli.profile.clone(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServerClientError {
    code: &'static str,
    message: String,
}

fn is_loopback_http_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1"
}

fn validate_namespace_limit(limit: u32) -> Result<(), ApiError> {
    if (1..=MAX_NAMESPACE_LIMIT).contains(&limit) {
        Ok(())
    } else {
        Err(ApiError::new(
            "invalid_limit",
            format!("limit must be between 1 and {MAX_NAMESPACE_LIMIT}"),
        ))
    }
}

fn validate_node_id_query_value(value: &str) -> Result<&str, ApiError> {
    let value = value.trim();
    let is_valid = !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'));
    if is_valid {
        Ok(value)
    } else {
        Err(ApiError::new(
            "invalid_parent_node_id",
            "parent node IDs may only contain ASCII letters, numbers, '.', '_', '-', or ':'",
        ))
    }
}

fn server_get_json(
    server_url: &str,
    path: &str,
    bearer_token: Option<&str>,
) -> Result<(u16, serde_json::Value), ServerClientError> {
    let endpoint = parse_http_endpoint(server_url, path)?;
    if bearer_token.is_some() && !is_loopback_http_host(&endpoint.host) {
        return Err(ServerClientError {
            code: "insecure_server_url",
            message: "server bearer tokens may only be sent to loopback HTTP URLs until HTTPS support lands"
                .to_string(),
        });
    }
    let addresses = (endpoint.host.as_str(), endpoint.port)
        .to_socket_addrs()
        .map_err(|error| ServerClientError {
            code: "server_unavailable",
            message: format!("could not resolve BiohazardFS server: {error}"),
        })?;
    let require_loopback = bearer_token.is_some();
    let mut saw_address = false;
    let mut saw_loopback_address = false;
    let mut last_error = None;
    let mut stream = None;
    for address in addresses {
        saw_address = true;
        if require_loopback && !address.ip().is_loopback() {
            continue;
        }
        saw_loopback_address = true;
        match TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        if require_loopback && saw_address && !saw_loopback_address {
            ServerClientError {
                code: "insecure_server_url",
                message: "server bearer tokens may only be sent to resolved loopback addresses"
                    .to_string(),
            }
        } else {
            ServerClientError {
                code: "server_unavailable",
                message: match last_error {
                    Some(error) => format!("could not connect to BiohazardFS server: {error}"),
                    None => "could not resolve BiohazardFS server address".to_string(),
                },
            }
        }
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(server_io_error)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(server_io_error)?;

    let auth_header = bearer_token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "GET {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\n{}Connection: close\r\n\r\n",
        endpoint.path, endpoint.host_header, auth_header
    )
    .map_err(server_io_error)?;
    stream.flush().map_err(server_io_error)?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(server_io_error)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| ServerClientError {
            code: "server_protocol_error",
            message: "server response did not include HTTP headers".to_string(),
        })?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse::<u16>().ok())
        .ok_or_else(|| ServerClientError {
            code: "server_protocol_error",
            message: "server response did not include a valid HTTP status".to_string(),
        })?;
    let payload =
        serde_json::from_str::<serde_json::Value>(body).map_err(|error| ServerClientError {
            code: "server_protocol_error",
            message: format!("server response was not valid JSON: {error}"),
        })?;
    Ok((status, payload))
}

fn server_io_error(error: std::io::Error) -> ServerClientError {
    ServerClientError {
        code: "server_unavailable",
        message: format!("BiohazardFS server request failed: {error}"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HttpEndpoint {
    host: String,
    host_header: String,
    port: u16,
    path: String,
}

fn parse_http_endpoint(server_url: &str, path: &str) -> Result<HttpEndpoint, ServerClientError> {
    let rest = server_url
        .trim()
        .strip_prefix("http://")
        .ok_or_else(|| ServerClientError {
            code: "invalid_server_url",
            message: "server URL must start with http:// in the current MVP client".to_string(),
        })?;
    let (authority, base_path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port, host_header) = parse_http_authority(authority)?;
    if host.trim().is_empty() {
        return Err(ServerClientError {
            code: "invalid_server_url",
            message: "server URL host is empty".to_string(),
        });
    }

    let base_path = base_path.trim_matches('/');
    let request_path = path.trim_start_matches('/');
    let full_path = if base_path.is_empty() {
        format!("/{request_path}")
    } else {
        format!("/{base_path}/{request_path}")
    };

    Ok(HttpEndpoint {
        host,
        host_header,
        port,
        path: full_path,
    })
}

fn parse_http_authority(authority: &str) -> Result<(String, u16, String), ServerClientError> {
    if let Some(without_opening_bracket) = authority.strip_prefix('[') {
        let (host, after_host) =
            without_opening_bracket
                .split_once(']')
                .ok_or_else(|| ServerClientError {
                    code: "invalid_server_url",
                    message: "server URL IPv6 host is missing a closing bracket".to_string(),
                })?;
        let port = match after_host.strip_prefix(':') {
            Some(port) => port.parse::<u16>().map_err(|_| ServerClientError {
                code: "invalid_server_url",
                message: "server URL port is not valid".to_string(),
            })?,
            None if after_host.is_empty() => 80,
            None => {
                return Err(ServerClientError {
                    code: "invalid_server_url",
                    message: "server URL IPv6 host has invalid authority syntax".to_string(),
                });
            }
        };
        return Ok((host.to_string(), port, authority.to_string()));
    }

    match authority.rsplit_once(':') {
        Some((host, port)) => {
            let port = port.parse::<u16>().map_err(|_| ServerClientError {
                code: "invalid_server_url",
                message: "server URL port is not valid".to_string(),
            })?;
            Ok((host.to_string(), port, authority.to_string()))
        }
        None => Ok((authority.to_string(), 80, authority.to_string())),
    }
}

fn daemon_exit_code(code: &str) -> u8 {
    match code {
        "auth_required" => EXIT_AUTH,
        "invalid_daemon_endpoint" => EXIT_USAGE,
        _ => EXIT_DAEMON_UNAVAILABLE,
    }
}

fn daemon_error_code(error: &DaemonClientError) -> &'static str {
    match error {
        DaemonClientError::InvalidEndpoint(_) => "invalid_daemon_endpoint",
        DaemonClientError::MissingToken => "auth_required",
        DaemonClientError::Io(_) => "daemon_unavailable",
        DaemonClientError::Json(_) | DaemonClientError::Protocol(_) => "daemon_protocol_error",
        DaemonClientError::Daemon(api_error) if api_error.code == "unauthorized" => "auth_required",
        DaemonClientError::Daemon(_) => "daemon_error",
    }
}

fn schema_json(command: SchemaCommand) -> String {
    let commands = vec![
        "client.status".to_string(),
        "daemon.status".to_string(),
        "daemon.methods".to_string(),
        "config.path".to_string(),
        "config.show".to_string(),
        "config.validate".to_string(),
        "namespace.children".to_string(),
        "schema.list".to_string(),
        "schema.command".to_string(),
    ];
    let (method, note) = match command {
        SchemaCommand::List => ("schema.list", "scaffold command registry".to_string()),
        SchemaCommand::Command { name } => ("schema.command", format!("schema stub for {name}")),
    };

    let envelope =
        CommandResponseEnvelope::ok(method, CommandSchemaSummary { commands, note }, Source::Cli);
    serde_json::to_string_pretty(&envelope).expect("schema summary serializes")
}

fn local_token() -> Option<String> {
    std::env::var(LOCAL_TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
}

fn server_token() -> Option<String> {
    std::env::var(SERVER_TOKEN_ENV)
        .ok()
        .filter(|token| !token.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_server_endpoint_with_base_path() {
        let endpoint = parse_http_endpoint(
            "http://127.0.0.1:8080/api",
            "/v1/namespace/children?limit=1",
        )
        .expect("endpoint parses");
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.host_header, "127.0.0.1:8080");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.path, "/api/v1/namespace/children?limit=1");
    }

    #[test]
    fn parses_bracketed_ipv6_loopback_endpoint() {
        let endpoint =
            parse_http_endpoint("http://[::1]:8080", "/readyz").expect("endpoint parses");
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.host_header, "[::1]:8080");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.path, "/readyz");
    }

    #[test]
    fn rejects_https_until_tls_client_lands() {
        let error = parse_http_endpoint("https://biohazardfs.example", "/readyz")
            .expect_err("https is not supported by the MVP stdlib client");
        assert_eq!(error.code, "invalid_server_url");
    }

    #[test]
    fn identifies_only_loopback_hosts_as_bearer_safe() {
        assert!(is_loopback_http_host("localhost"));
        assert!(is_loopback_http_host("127.0.0.1"));
        assert!(is_loopback_http_host("::1"));
        assert!(!is_loopback_http_host("192.168.1.128"));
        assert!(!is_loopback_http_host("biohazardfs.example"));
    }

    #[test]
    fn rejects_query_injection_in_parent_node_id() {
        let error = validate_node_id_query_value("node_root_dir&limit=500")
            .expect_err("query separators are not valid node IDs");
        assert_eq!(error.code, "invalid_parent_node_id");
    }

    #[test]
    fn rejects_empty_parent_node_id() {
        let error = validate_node_id_query_value("   ").expect_err("empty parent is invalid");
        assert_eq!(error.code, "invalid_parent_node_id");
    }

    #[test]
    fn validates_namespace_limit_against_server_contract() {
        assert!(validate_namespace_limit(1).is_ok());
        assert!(validate_namespace_limit(MAX_NAMESPACE_LIMIT).is_ok());
        assert_eq!(
            validate_namespace_limit(0)
                .expect_err("zero is invalid")
                .code,
            "invalid_limit"
        );
        assert_eq!(
            validate_namespace_limit(MAX_NAMESPACE_LIMIT + 1)
                .expect_err("too large is invalid")
                .code,
            "invalid_limit"
        );
    }
}
