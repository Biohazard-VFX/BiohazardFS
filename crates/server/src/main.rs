use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "biohazardfs-server")]
#[command(about = "BiohazardFS server/control-plane scaffold")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTP API server.
    Serve {
        /// Bind address for the HTTP API. Overrides BIOHAZARDFS_SERVER_BIND when provided.
        #[arg(long)]
        addr: Option<String>,
    },
    /// Run the worker scaffold once and print status.
    Worker,
    /// Apply server database migrations and print status.
    Migrate,
    /// Print health envelope and exit.
    Health,
    /// Print version envelope and exit.
    Version,
    /// Print the resolved redacted shared runtime config and exit.
    Config,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve { addr: None }) {
        Command::Serve { addr } => biohazardfs_server::serve(&resolve_bind_addr(addr)?),
        Command::Worker => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.worker",
            biohazardfs_server::worker_payload(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::Migrate => match biohazardfs_server::migrate_payload() {
            Ok(payload) => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
                "server.migrate",
                payload,
                biohazardfs_api_types::Source::Server,
            )),
            Err(error) => {
                let envelope =
                    biohazardfs_api_types::ServerResponseEnvelope::<serde_json::Value>::error(
                        "server.migrate",
                        error.into_api_error(),
                        biohazardfs_api_types::Source::Server,
                    );
                print_json(&envelope)?;
                std::process::exit(2);
            }
        },
        Command::Health => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.health",
            biohazardfs_server::server_health(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::Version => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.version",
            biohazardfs_server::server_version(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::Config => {
            match biohazardfs_core::config::RuntimeConfig::load(Default::default()) {
                Ok(loaded) => {
                    let mut envelope = biohazardfs_api_types::ServerResponseEnvelope::ok(
                        "server.config",
                        loaded.config.clone(),
                        biohazardfs_api_types::Source::Server,
                    );
                    envelope.warnings = loaded
                        .validation_warnings()
                        .into_iter()
                        .map(|warning| biohazardfs_api_types::Warning {
                            code: warning.code,
                            message: warning.message,
                        })
                        .collect();
                    print_json(&envelope)
                }
                Err(error) => {
                    let envelope =
                        biohazardfs_api_types::ServerResponseEnvelope::<serde_json::Value>::error(
                            "server.config",
                            biohazardfs_api_types::ApiError::new(error.code, error.message),
                            biohazardfs_api_types::Source::Server,
                        );
                    print_json(&envelope)?;
                    std::process::exit(2);
                }
            }
        }
    }
}

fn resolve_bind_addr(addr: Option<String>) -> std::io::Result<String> {
    match addr.filter(|value| !value.is_empty()) {
        Some(addr) => Ok(addr),
        None => default_bind_addr(),
    }
}

fn default_bind_addr() -> std::io::Result<String> {
    biohazardfs_core::config::RuntimeConfig::load(Default::default())
        .map(|loaded| loaded.config.server.bind)
        .map_err(std::io::Error::other)
}

fn print_json<T>(payload: &T) -> std::io::Result<()>
where
    T: serde::Serialize,
{
    println!(
        "{}",
        serde_json::to_string_pretty(payload).map_err(std::io::Error::other)?
    );
    Ok(())
}
