use std::path::PathBuf;
use std::process::ExitCode;

use biohazardfs_fuse::{MountConfig, mount_read_only_workspace};
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "biohazardfs-fuse")]
#[command(about = "BiohazardFS virtual filesystem adapter")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Mount a read-only virtual view of a local workspace/source tree.
    Mount {
        /// Existing source/workspace directory to expose through FUSE.
        #[arg(long)]
        source: PathBuf,
        /// Existing empty directory used as the FUSE mountpoint.
        #[arg(long)]
        mountpoint: PathBuf,
        /// Stay in the foreground. This is the current supported mode.
        #[arg(long, default_value_t = true)]
        foreground: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Mount {
            source,
            mountpoint,
            foreground,
        } => mount_read_only_workspace(MountConfig {
            source,
            mountpoint,
            foreground,
        }),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!(
                "biohazardfs-fuse error: {} ({:?})",
                error.message(),
                error.kind()
            );
            let mut source = std::error::Error::source(&error);
            while let Some(error) = source {
                eprintln!("caused by: {error}");
                source = error.source();
            }
            ExitCode::from(2)
        }
    }
}
