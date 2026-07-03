use std::path::PathBuf;

use biohazardfs_core::config::{ConfigError, ConfigLoadOptions, LoadedConfig, RuntimeConfig};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "biohazardfs-server")]
#[command(about = "BiohazardFS server/control-plane scaffold")]
struct Cli {
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
    /// Check or initialize the configured RustFS/S3-compatible object-store bucket.
    ObjectStore {
        #[command(subcommand)]
        command: ObjectStoreCommand,
    },
    /// Print the resolved redacted shared runtime config and exit.
    Config,
}

#[derive(Debug, Subcommand)]
enum ObjectStoreCommand {
    /// Check that the configured bucket exists and credentials work.
    Check,
    /// Idempotently create the configured bucket when it is missing, then report status.
    EnsureBucket,
}

fn main() -> std::io::Result<()> {
    let mut cli = Cli::parse();
    let command = cli.command.take().unwrap_or(Command::Serve { addr: None });
    match command {
        Command::Serve { addr } => {
            let loaded = load_runtime_config_or_exit(&cli, "server.serve")?;
            let bind = resolve_bind_addr(addr, &loaded.config);
            biohazardfs_server::serve_with_config(&bind, loaded.config)
        }
        Command::Worker => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.worker",
            biohazardfs_server::worker_payload(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::Migrate => {
            let loaded = load_runtime_config_or_exit(&cli, "server.migrate")?;
            match biohazardfs_server::migrate_payload_with_config(&loaded.config) {
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
            }
        }
        Command::Health => {
            let loaded = load_runtime_config_or_exit(&cli, "server.health")?;
            print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
                "server.health",
                biohazardfs_server::server_health_with_config(&loaded.config),
                biohazardfs_api_types::Source::Server,
            ))
        }
        Command::Version => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.version",
            biohazardfs_server::server_version(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::ObjectStore { command } => {
            let loaded = load_runtime_config_or_exit(&cli, "server.object_store")?;
            let result = match command {
                ObjectStoreCommand::Check => {
                    biohazardfs_server::object_store_check_payload_with_config(&loaded.config)
                }
                ObjectStoreCommand::EnsureBucket => {
                    biohazardfs_server::object_store_ensure_bucket_payload_with_config(
                        &loaded.config,
                    )
                }
            };
            match result {
                Ok(payload) => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
                    "server.object_store",
                    payload,
                    biohazardfs_api_types::Source::Server,
                )),
                Err(error) => {
                    let envelope =
                        biohazardfs_api_types::ServerResponseEnvelope::<serde_json::Value>::error(
                            "server.object_store",
                            error.into_api_error(),
                            biohazardfs_api_types::Source::Server,
                        );
                    print_json(&envelope)?;
                    std::process::exit(2);
                }
            }
        }
        Command::Config => match RuntimeConfig::load(config_load_options(&cli)) {
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
        },
    }
}

fn config_load_options(cli: &Cli) -> ConfigLoadOptions {
    ConfigLoadOptions {
        config_file: cli.config_file.clone(),
        profile: cli.profile.clone(),
    }
}

fn load_runtime_config_or_exit(cli: &Cli, operation: &str) -> std::io::Result<LoadedConfig> {
    match RuntimeConfig::load(config_load_options(cli)) {
        Ok(loaded) => Ok(loaded),
        Err(error) => {
            print_json(&config_error_envelope(operation, error))?;
            std::process::exit(2);
        }
    }
}

fn config_error_envelope(
    operation: &str,
    error: ConfigError,
) -> biohazardfs_api_types::ServerResponseEnvelope<serde_json::Value> {
    biohazardfs_api_types::ServerResponseEnvelope::<serde_json::Value>::error(
        operation,
        biohazardfs_api_types::ApiError::new(error.code, error.message),
        biohazardfs_api_types::Source::Server,
    )
}

fn resolve_bind_addr(addr: Option<String>, config: &RuntimeConfig) -> String {
    match addr.filter(|value| !value.is_empty()) {
        Some(addr) => addr,
        None => config.server.bind.clone(),
    }
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
