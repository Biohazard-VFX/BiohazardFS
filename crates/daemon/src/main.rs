use clap::Parser;

#[derive(Debug, Parser)]
#[command(name = "biohazardfsd")]
#[command(about = "BiohazardFS local daemon scaffold")]
struct Args {
    /// Enable development/test loopback HTTP transport. Production transport will be platform IPC.
    #[arg(long)]
    dev_loopback_http: bool,

    /// Development/test loopback HTTP address.
    #[arg(long, default_value = biohazardfs_daemon::default_dev_loopback_endpoint())]
    addr: String,
}

fn main() -> std::io::Result<()> {
    let args = Args::parse();

    if !args.dev_loopback_http {
        eprintln!(
            "biohazardfsd scaffold currently implements only --dev-loopback-http; production IPC is not implemented yet"
        );
        std::process::exit(2);
    }

    let local_token = match std::env::var(biohazardfs_daemon::LOCAL_TOKEN_ENV) {
        Ok(token) if !token.is_empty() => token,
        _ => {
            eprintln!(
                "missing local daemon token; set {}",
                biohazardfs_daemon::LOCAL_TOKEN_ENV
            );
            std::process::exit(2);
        }
    };

    biohazardfs_daemon::run_dev_loopback_http(biohazardfs_daemon::DevLoopbackConfig::new(
        args.addr,
        local_token,
    ))
}
