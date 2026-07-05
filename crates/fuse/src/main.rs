use std::path::PathBuf;
use std::process::ExitCode;

use biohazardfs_fuse::{
    FuseErrorKind, MountConfig, WorkspaceMountConfig, mount_read_only_workspace, mount_workspace,
};
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
    /// Mount a read-write BiohazardFS workspace backed by the local daemon.
    ///
    /// Files hydrate on open via `file.read`; writes buffer per file handle
    /// and push one complete blob per flush/fsync via `file.write`.
    MountWorkspace {
        /// Loopback daemon endpoint, e.g. `127.0.0.1:47666`.
        #[arg(long)]
        daemon_endpoint: String,
        /// Owner-only local daemon session token (or set BIOHAZARDFS_LOCAL_TOKEN).
        #[arg(long, env = "BIOHAZARDFS_LOCAL_TOKEN")]
        local_token: String,
        /// Local cache directory for hydrated content. Created if missing.
        #[arg(long)]
        cache_dir: PathBuf,
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
        Command::MountWorkspace {
            daemon_endpoint,
            local_token,
            cache_dir,
            mountpoint,
            foreground,
        } => mount_workspace(WorkspaceMountConfig {
            daemon_endpoint,
            local_token,
            cache_dir,
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
            if matches!(error.kind(), &FuseErrorKind::UnsupportedPlatform) {
                ExitCode::from(3)
            } else {
                ExitCode::from(2)
            }
        }
    }
}
