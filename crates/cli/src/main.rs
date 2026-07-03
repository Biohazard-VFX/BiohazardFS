use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
use sha2::{Digest, Sha256};

const EXIT_OK: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_AUTH: u8 = 3;
const EXIT_NOT_FOUND: u8 = 4;
const EXIT_DAEMON_UNAVAILABLE: u8 = 6;
const EXIT_SERVER_UNAVAILABLE: u8 = 6;
const SERVER_TOKEN_ENV: &str = "BIOHAZARDFS_SERVER_TOKEN";
const MAX_NAMESPACE_LIMIT: u32 = 500;
const MAX_CONTENT_OBJECT_BYTES: usize = 1024 * 1024;
const MAX_SERVER_JSON_RESPONSE_BYTES: u64 = 3 * 1024 * 1024;

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
    /// Server-backed content object transfer commands.
    Object {
        #[command(subcommand)]
        command: ObjectCommand,
    },
    /// Server-backed file workflow commands.
    File {
        #[command(subcommand)]
        command: FileCommand,
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
    /// Show local workspace runtime status from the daemon.
    WorkspaceStatus,
    /// List a local workspace directory through the daemon.
    WorkspaceList {
        /// Relative workspace path. Must stay inside the workspace root.
        #[arg(long, default_value = "")]
        path: String,
    },
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
enum FileCommand {
    /// Upload a local file and record/update a metadata file node.
    Put {
        /// Local file path to upload.
        path: PathBuf,
        /// Optional parent directory node ID. Omitted writes a root file.
        #[arg(long)]
        parent: Option<String>,
        /// Optional BiohazardFS file name. Defaults to the local file name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Download the current content of a metadata file node.
    Get {
        /// File node ID returned by file put or namespace children.
        #[arg(long, alias = "node-id")]
        node: String,
        /// Local output file path to write; existing paths are not overwritten.
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ObjectCommand {
    /// Upload a local file as a content-addressed object.
    Put {
        /// Local file path to upload. Secrets should not be embedded in paths.
        path: PathBuf,
    },
    /// Download a content-addressed object to a local file.
    Get {
        /// SHA-256 content hash returned by object put.
        #[arg(long)]
        sha256: String,
        /// Local output file path to write.
        #[arg(long)]
        output: PathBuf,
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
        Command::Daemon {
            command: DaemonCommand::WorkspaceStatus,
        } => daemon_rpc_json(
            &cli,
            "daemon.workspace.status",
            "workspace.status",
            serde_json::json!({}),
        ),
        Command::Daemon {
            command: DaemonCommand::WorkspaceList { path },
        } => daemon_rpc_json(
            &cli,
            "daemon.workspace.list",
            "workspace.list",
            serde_json::json!({ "path": path }),
        ),
        Command::Config { command } => config_json(&cli, command),
        Command::Namespace { command } => namespace_json(&cli, command),
        Command::Object { command } => object_json(&cli, command),
        Command::File { command } => file_json(&cli, command),
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

fn daemon_rpc_json(
    cli: &Cli,
    command_name: &'static str,
    method: &'static str,
    params: serde_json::Value,
) -> (String, u8) {
    let Some(token) = local_token() else {
        let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
            command_name,
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
    let mut request = DaemonRequest::new(method, Source::Cli);
    request.params = params;
    match client.call::<serde_json::Value>(&request) {
        Ok(envelope) if envelope.ok => {
            let output = CommandResponseEnvelope::ok(
                command_name,
                envelope.data.unwrap_or_else(|| serde_json::json!({})),
                Source::Cli,
            );
            (
                serde_json::to_string_pretty(&output).expect("daemon rpc serializes"),
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
            } else if normalized_error.code.ends_with("not_found") {
                EXIT_NOT_FOUND
            } else {
                EXIT_DAEMON_UNAVAILABLE
            };
            let output: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error(command_name, normalized_error, Source::Cli);
            (
                serde_json::to_string_pretty(&output).expect("daemon error serializes"),
                exit_code,
            )
        }
        Err(error) => {
            let code = daemon_error_code(&error);
            let output: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
                command_name,
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

fn file_json(cli: &Cli, command: FileCommand) -> (String, u8) {
    match command {
        FileCommand::Put { path, parent, name } => file_put_json(cli, path, parent, name),
        FileCommand::Get { node, output } => file_get_json(cli, node, output),
    }
}

fn file_put_json(
    cli: &Cli,
    path: PathBuf,
    parent: Option<String>,
    name: Option<String>,
) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json("file.put", "file APIs");
    };
    let file_name = match resolve_file_name(&path, name.as_deref()) {
        Ok(name) => name,
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("file.put", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("file put name error serializes"),
                EXIT_USAGE,
            );
        }
    };
    let parent = match parent
        .as_deref()
        .map(validate_node_id_query_value)
        .transpose()
    {
        Ok(parent) => parent.map(str::to_string),
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("file.put", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("file put parent error serializes"),
                EXIT_USAGE,
            );
        }
    };
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json("file.put", error),
    };
    let content = match read_bounded_input_file(&path) {
        Ok(content) => content,
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("file.put", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("file put read error serializes"),
                EXIT_USAGE,
            );
        }
    };
    let local_hash = sha256_hex(&content);
    let local_size = content.len() as u64;
    let mut request_path = format!(
        "/api/v1/files/content?name={}",
        percent_encode_query_value(&file_name)
    );
    if let Some(parent) = parent.as_deref() {
        request_path.push_str("&parent_node_id=");
        request_path.push_str(&percent_encode_query_value(parent));
    }
    request_path.push_str("&source=cli");

    match server_request_json(
        "PUT",
        &loaded.config.server.public_url,
        &request_path,
        Some(&token),
        &content,
    ) {
        Ok((_status, payload)) if payload.get("ok").and_then(|ok| ok.as_bool()) == Some(true) => {
            let mut data = payload
                .get("data")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let server_hash = data.get("content_hash").and_then(|value| value.as_str());
            let server_size = data.get("size_bytes").and_then(|value| value.as_u64());
            if server_hash != Some(local_hash.as_str()) || server_size != Some(local_size) {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "file.put",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not match uploaded file hash and size",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("file put protocol error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "input_path".to_string(),
                    serde_json::Value::String(path.to_string_lossy().to_string()),
                );
            }
            let envelope = CommandResponseEnvelope::ok("file.put", data, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("file put serializes"),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json("file.put", status, payload),
        Err(error) => server_client_error_json("file.put", error),
    }
}

fn file_get_json(cli: &Cli, node: String, output: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json("file.get", "file APIs");
    };
    let node = match validate_node_id_query_value(&node) {
        Ok(node) => node.to_string(),
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("file.get", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("file get node error serializes"),
                EXIT_USAGE,
            );
        }
    };
    if fs::symlink_metadata(&output).is_ok() {
        let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
            "file.get",
            ApiError::new(
                "output_exists",
                "output path already exists; refusing to overwrite without an explicit overwrite command",
            ),
            Source::Cli,
        );
        return (
            serde_json::to_string_pretty(&envelope).expect("file get exists error serializes"),
            EXIT_USAGE,
        );
    }
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json("file.get", error),
    };
    let request_path = format!(
        "/api/v1/files/content?node_id={}",
        percent_encode_query_value(&node)
    );
    match server_request_json(
        "GET",
        &loaded.config.server.public_url,
        &request_path,
        Some(&token),
        &[],
    ) {
        Ok((_status, payload)) if payload.get("ok").and_then(|ok| ok.as_bool()) == Some(true) => {
            let mut data = payload
                .get("data")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let Some(content_hex) = data.get("content_hex").and_then(|value| value.as_str()) else {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "file.get",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not include content_hex",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("file get protocol error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            };
            let content = match hex_to_bytes(content_hex) {
                Ok(content) => content,
                Err(error) => {
                    let envelope: CommandResponseEnvelope<serde_json::Value> =
                        CommandResponseEnvelope::error("file.get", error, Source::Cli);
                    return (
                        serde_json::to_string_pretty(&envelope)
                            .expect("file get decode error serializes"),
                        EXIT_SERVER_UNAVAILABLE,
                    );
                }
            };
            let server_hash = data
                .get("content_hash")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if sha256_hex(&content) != server_hash {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "file.get",
                        ApiError::new(
                            "content_hash_mismatch",
                            "downloaded file did not match server hash",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("file get hash error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Err(error) = write_file_atomically(&output, &content) {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "file.get",
                        ApiError::new(
                            "file_write_failed",
                            format!("could not write output file: {error}"),
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("file get write error serializes"),
                    EXIT_USAGE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.remove("content_hex");
                object.insert(
                    "output_path".to_string(),
                    serde_json::Value::String(output.to_string_lossy().to_string()),
                );
            }
            let envelope = CommandResponseEnvelope::ok("file.get", data, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("file get serializes"),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json("file.get", status, payload),
        Err(error) => server_client_error_json("file.get", error),
    }
}

fn object_json(cli: &Cli, command: ObjectCommand) -> (String, u8) {
    match command {
        ObjectCommand::Put { path } => object_put_json(cli, path),
        ObjectCommand::Get { sha256, output } => object_get_json(cli, sha256, output),
    }
}

fn object_put_json(cli: &Cli, path: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json("object.put", "content object APIs");
    };
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json("object.put", error),
    };
    let content = match read_bounded_input_file(&path) {
        Ok(content) => content,
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("object.put", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("object put read error serializes"),
                EXIT_USAGE,
            );
        }
    };

    let local_hash = sha256_hex(&content);
    let local_size = content.len() as u64;
    match server_request_json(
        "PUT",
        &loaded.config.server.public_url,
        "/api/v1/objects/content",
        Some(&token),
        &content,
    ) {
        Ok((_status, payload)) if payload.get("ok").and_then(|ok| ok.as_bool()) == Some(true) => {
            let mut data = payload
                .get("data")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let server_hash = data.get("content_hash").and_then(|value| value.as_str());
            let server_size = data.get("size_bytes").and_then(|value| value.as_u64());
            if server_hash != Some(local_hash.as_str()) || server_size != Some(local_size) {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "object.put",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not match uploaded content hash and size",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("object put protocol error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "input_path".to_string(),
                    serde_json::Value::String(path.to_string_lossy().to_string()),
                );
            }
            let envelope = CommandResponseEnvelope::ok("object.put", data, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("object put serializes"),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json("object.put", status, payload),
        Err(error) => server_client_error_json("object.put", error),
    }
}

fn object_get_json(cli: &Cli, sha256: String, output: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json("object.get", "content object APIs");
    };
    let sha256 = match validate_content_hash(&sha256) {
        Ok(hash) => hash,
        Err(error) => {
            let envelope: CommandResponseEnvelope<serde_json::Value> =
                CommandResponseEnvelope::error("object.get", error, Source::Cli);
            return (
                serde_json::to_string_pretty(&envelope).expect("object get hash error serializes"),
                EXIT_USAGE,
            );
        }
    };
    if fs::symlink_metadata(&output).is_ok() {
        let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
            "object.get",
            ApiError::new(
                "output_exists",
                "output path already exists; refusing to overwrite without an explicit overwrite command",
            ),
            Source::Cli,
        );
        return (
            serde_json::to_string_pretty(&envelope).expect("object get exists error serializes"),
            EXIT_USAGE,
        );
    }
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json("object.get", error),
    };

    let path = format!("/api/v1/objects/content?sha256={sha256}");
    match server_request_json(
        "GET",
        &loaded.config.server.public_url,
        &path,
        Some(&token),
        &[],
    ) {
        Ok((_status, payload)) if payload.get("ok").and_then(|ok| ok.as_bool()) == Some(true) => {
            let mut data = payload
                .get("data")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({}));
            let Some(content_hex) = data.get("content_hex").and_then(|value| value.as_str()) else {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "object.get",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not include content_hex",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("object get protocol error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            };
            let content = match hex_to_bytes(content_hex) {
                Ok(content) => content,
                Err(error) => {
                    let envelope: CommandResponseEnvelope<serde_json::Value> =
                        CommandResponseEnvelope::error("object.get", error, Source::Cli);
                    return (
                        serde_json::to_string_pretty(&envelope)
                            .expect("object get decode error serializes"),
                        EXIT_SERVER_UNAVAILABLE,
                    );
                }
            };
            if sha256_hex(&content) != sha256 {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "object.get",
                        ApiError::new(
                            "content_hash_mismatch",
                            "downloaded content did not match requested hash",
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("object get hash error serializes"),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Err(error) = write_file_atomically(&output, &content) {
                let envelope: CommandResponseEnvelope<serde_json::Value> =
                    CommandResponseEnvelope::error(
                        "object.get",
                        ApiError::new(
                            "file_write_failed",
                            format!("could not write output file: {error}"),
                        ),
                        Source::Cli,
                    );
                return (
                    serde_json::to_string_pretty(&envelope)
                        .expect("object get write error serializes"),
                    EXIT_USAGE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.remove("content_hex");
                object.insert(
                    "output_path".to_string(),
                    serde_json::Value::String(output.to_string_lossy().to_string()),
                );
            }
            let envelope = CommandResponseEnvelope::ok("object.get", data, Source::Cli);
            (
                serde_json::to_string_pretty(&envelope).expect("object get serializes"),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json("object.get", status, payload),
        Err(error) => server_client_error_json("object.get", error),
    }
}

fn auth_required_json(command: &'static str, api_name: &str) -> (String, u8) {
    let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
        command,
        ApiError::new(
            "auth_required",
            format!("set {SERVER_TOKEN_ENV} to call BiohazardFS server {api_name}"),
        ),
        Source::Cli,
    );
    (
        serde_json::to_string_pretty(&envelope).expect("auth error serializes"),
        EXIT_AUTH,
    )
}

fn server_error_json(
    command: &'static str,
    status: u16,
    payload: serde_json::Value,
) -> (String, u8) {
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
    } else if status == 404
        || matches!(
            error.code.as_str(),
            "not_found" | "content_object_not_found" | "file_not_found" | "parent_not_found"
        )
    {
        EXIT_NOT_FOUND
    } else if status == 400 || status == 413 {
        EXIT_USAGE
    } else {
        EXIT_SERVER_UNAVAILABLE
    };
    let envelope: CommandResponseEnvelope<serde_json::Value> =
        CommandResponseEnvelope::error(command, error, Source::Cli);
    (
        serde_json::to_string_pretty(&envelope).expect("server error serializes"),
        exit_code,
    )
}

fn server_client_error_json(command: &'static str, error: ServerClientError) -> (String, u8) {
    let exit_code = if matches!(error.code, "invalid_server_url" | "insecure_server_url") {
        EXIT_USAGE
    } else {
        EXIT_SERVER_UNAVAILABLE
    };
    let envelope: CommandResponseEnvelope<serde_json::Value> = CommandResponseEnvelope::error(
        command,
        ApiError::new(error.code, error.message),
        Source::Cli,
    );
    (
        serde_json::to_string_pretty(&envelope).expect("server client error serializes"),
        exit_code,
    )
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

fn resolve_file_name(path: &Path, explicit_name: Option<&str>) -> Result<String, ApiError> {
    let name = explicit_name
        .map(str::to_string)
        .or_else(|| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .ok_or_else(|| {
            ApiError::new("file_name_required", "could not infer file name from path")
        })?;
    validate_file_name(&name)?;
    Ok(name)
}

fn validate_file_name(name: &str) -> Result<(), ApiError> {
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
        Err(ApiError::new(
            "invalid_file_name",
            "file name is not valid for the MVP file API",
        ))
    }
}

fn percent_encode_query_value(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn read_bounded_input_file(path: &Path) -> Result<Vec<u8>, ApiError> {
    let metadata = fs::metadata(path).map_err(|error| {
        ApiError::new(
            "file_read_failed",
            format!("could not inspect input file: {error}"),
        )
    })?;
    if !metadata.is_file() {
        return Err(ApiError::new(
            "file_type_unsupported",
            "input path must be a regular file",
        ));
    }
    if metadata.len() > MAX_CONTENT_OBJECT_BYTES as u64 {
        return Err(ApiError::new(
            "content_too_large",
            "input file exceeds the MVP content upload limit",
        ));
    }
    let file = File::open(path).map_err(|error| {
        ApiError::new(
            "file_read_failed",
            format!("could not read input file: {error}"),
        )
    })?;
    let mut content = Vec::new();
    file.take(MAX_CONTENT_OBJECT_BYTES as u64 + 1)
        .read_to_end(&mut content)
        .map_err(|error| {
            ApiError::new(
                "file_read_failed",
                format!("could not read input file: {error}"),
            )
        })?;
    if content.len() > MAX_CONTENT_OBJECT_BYTES {
        return Err(ApiError::new(
            "content_too_large",
            "input file exceeds the MVP content upload limit",
        ));
    }
    Ok(content)
}

fn write_file_atomically(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("biohazardfs-output");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = parent.join(format!(
        ".{file_name}.biohazardfs-tmp-{}-{nonce}",
        std::process::id()
    ));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temp_path, path)?;
        fs::remove_file(&temp_path)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    write_result
}

fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

fn validate_content_hash(value: &str) -> Result<String, ApiError> {
    let value = value.trim().to_ascii_lowercase();
    let is_valid = value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit());
    if is_valid {
        Ok(value)
    } else {
        Err(ApiError::new(
            "invalid_content_hash",
            "content hash must be a 64-character SHA-256 hex digest",
        ))
    }
}

fn hex_to_bytes(value: &str) -> Result<Vec<u8>, ApiError> {
    if !value.len().is_multiple_of(2) || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::new(
            "invalid_content_encoding",
            "server returned invalid content hex",
        ));
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| {
                ApiError::new(
                    "invalid_content_encoding",
                    "server returned invalid content hex",
                )
            })
        })
        .collect()
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
    server_request_json("GET", server_url, path, bearer_token, &[])
}

fn server_request_json(
    method: &'static str,
    server_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: &[u8],
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
        "{} {} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\n{}Content-Length: {}\r\nConnection: close\r\n\r\n",
        method,
        endpoint.path,
        endpoint.host_header,
        auth_header,
        body.len()
    )
    .map_err(server_io_error)?;
    if !body.is_empty() {
        stream.write_all(body).map_err(server_io_error)?;
    }
    stream.flush().map_err(server_io_error)?;

    let mut response = String::new();
    stream
        .take(MAX_SERVER_JSON_RESPONSE_BYTES + 1)
        .read_to_string(&mut response)
        .map_err(server_io_error)?;
    if response.len() as u64 > MAX_SERVER_JSON_RESPONSE_BYTES {
        return Err(ServerClientError {
            code: "server_protocol_error",
            message: "server JSON response exceeded the MVP client limit".to_string(),
        });
    }
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
        "daemon.workspace.status".to_string(),
        "daemon.workspace.list".to_string(),
        "config.path".to_string(),
        "config.show".to_string(),
        "config.validate".to_string(),
        "namespace.children".to_string(),
        "object.put".to_string(),
        "object.get".to_string(),
        "file.put".to_string(),
        "file.get".to_string(),
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
