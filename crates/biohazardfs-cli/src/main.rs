use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "biohazardfs")]
#[command(about = "BiohazardFS virtual sync client")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print placeholder command schema.
    Commands,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Some(Command::Commands) => {
            println!("{}", serde_json::json!({"commands": [], "status": "scaffold"}));
        }
        None => {
            println!("{}", serde_json::json!({"name": "biohazardfs", "status": "scaffold"}));
        }
    }
}
