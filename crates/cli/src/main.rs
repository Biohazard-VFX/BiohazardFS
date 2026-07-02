use std::process::ExitCode;

use biohazardfs_api_types::{
    ApiError, ClientStatus, CommandResponseEnvelope, CommandSchemaSummary,
    DEV_LOOPBACK_HTTP_ENDPOINT, DaemonRequest, DaemonStatus, Source,
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
