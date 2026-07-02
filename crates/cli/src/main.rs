use std::path::PathBuf;
use std::process::ExitCode;

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
