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
    /// Run migration scaffold once and print status.
    Migrate,
    /// Print health envelope and exit.
    Health,
    /// Print version envelope and exit.
    Version,
}

fn main() -> std::io::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve { addr: None }) {
        Command::Serve { addr } => biohazardfs_server::serve(&resolve_bind_addr(addr)),
        Command::Worker => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.worker",
            biohazardfs_server::worker_payload(),
            biohazardfs_api_types::Source::Server,
        )),
        Command::Migrate => print_json(&biohazardfs_api_types::ServerResponseEnvelope::ok(
            "server.migrate",
            biohazardfs_server::migrate_payload(),
            biohazardfs_api_types::Source::Server,
        )),
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
    }
}

fn resolve_bind_addr(addr: Option<String>) -> String {
    addr.filter(|value| !value.is_empty())
        .unwrap_or_else(default_bind_addr)
}

fn default_bind_addr() -> String {
    std::env::var("BIOHAZARDFS_SERVER_BIND")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| biohazardfs_server::DEFAULT_BIND_ADDR.to_string())
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
