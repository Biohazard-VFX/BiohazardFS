use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use biohazardfs_api_types::{
    ApiError, COMMAND_SCHEMA_VERSION, ClientStatus, CommandResponseEnvelope, DAEMON_SCHEMA_VERSION,
    DEV_LOOPBACK_HTTP_ENDPOINT, DaemonRequest, EVENT_SCHEMA_VERSION, MutationClassification,
    OperationToken, SERVER_SCHEMA_VERSION, Source, Warning,
    known_methods::{self, Surface},
};
use biohazardfs_core::config::{
    CONFIG_SCHEMA_VERSION, ConfigError, ConfigLoadOptions, ENV_PROFILE, LoadedConfig,
    RuntimeConfig, resolve_config_file_path,
};
use biohazardfs_daemon::{DaemonClientError, DaemonHttpClient, LOCAL_TOKEN_ENV};
use clap::{Parser, Subcommand, ValueEnum};
use serde_json::Value;
use sha2::{Digest, Sha256};

// Exit codes — see docs/reference/COMMANDS.md "Exit codes".
const EXIT_OK: u8 = 0;
const EXIT_GENERAL: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_AUTH: u8 = 3;
const EXIT_NOT_FOUND: u8 = 4;
const EXIT_CONFLICT: u8 = 5;
const EXIT_DAEMON_UNAVAILABLE: u8 = 6;
const EXIT_SERVER_UNAVAILABLE: u8 = 6;
const EXIT_CONFIRMATION_REQUIRED: u8 = 7;
const EXIT_UNSUPPORTED_PLATFORM: u8 = 8;

const SERVER_TOKEN_ENV: &str = "BIOHAZARDFS_SERVER_TOKEN";
const MAX_NAMESPACE_LIMIT: u32 = 500;
const MAX_CONTENT_OBJECT_BYTES: usize = 1024 * 1024;
const MAX_SERVER_JSON_RESPONSE_BYTES: u64 = 3 * 1024 * 1024;
const DRY_RUN_TOKEN_TTL_SECONDS: u64 = 15 * 60;
const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Field keys whose arrays the ndjson renderer streams as one envelope per item.
const NDJSON_LIST_KEYS: &[&str] = &[
    "items",
    "entries",
    "events",
    "audit_events",
    "nodes",
    "children",
    "transfers",
    "snapshots",
    "locks",
    "conflicts",
    "worksets",
    "versions",
    "grants",
    "shares",
    "invites",
    "publishes",
    "mounts",
    "devices",
    "users",
    "methods",
    "schemas",
    "checks",
];

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

    /// Output format. JSON is the contract format; ndjson streams list-style responses; text is a human fallback.
    #[arg(long, global = true, value_enum, default_value_t = OutputFormat::Json, value_name = "FORMAT")]
    output: OutputFormat,

    /// Field mask (comma list or parenthesized grammar). Applied to list-style reads when supported.
    #[arg(long, global = true, value_name = "LIST")]
    fields: Option<String>,

    /// Pagination cursor returned by a previous list-style response.
    #[arg(long, global = true, value_name = "CURSOR")]
    cursor: Option<String>,

    /// Provenance source label for the request envelope and daemon RPC meta.
    #[arg(long, global = true, value_enum, value_name = "SOURCE")]
    source: Option<SourceArg>,

    /// Explicit request id. Generated when omitted.
    #[arg(long = "request-id", global = true, value_name = "ID")]
    request_id: Option<String>,

    /// Plan a mutating operation and return an operation token without applying it.
    #[arg(long = "dry-run", global = true)]
    dry_run: bool,

    /// Apply a mutating operation, bypassing the agent-safe confirmation gate.
    #[arg(long = "yes", global = true)]
    yes: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
#[value(rename_all = "kebab-case")]
enum OutputFormat {
    #[default]
    Json,
    Ndjson,
    Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
enum SourceArg {
    Ui,
    Cli,
    Agent,
    Api,
    Server,
    Test,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Show client and daemon reachability status.
    Status,
    /// Show product, schema, and daemon/server version stamps.
    Version,
    /// Diagnose the local install (config file, daemon reachability, deep checks optional).
    Doctor {
        /// Request deep diagnostics. Currently a stub; shallow checks still run.
        #[arg(long = "json-deep")]
        json_deep: bool,
    },
    /// Run validation smoke tests.
    Smoke {
        #[command(subcommand)]
        command: SmokeCommand,
    },
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
    /// Mount lifecycle commands.
    Mount {
        #[command(subcommand)]
        command: MountCommand,
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
    /// File workflow commands. `put`/`get` are server content primitives; the rest are daemon-backed.
    File {
        #[command(subcommand)]
        command: FileCommand,
    },
    /// Local cache commands.
    Cache {
        #[command(subcommand)]
        command: CacheCommand,
    },
    /// Transfer queue commands.
    Transfer {
        #[command(subcommand)]
        command: TransferCommand,
    },
    /// Snapshot commands.
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCommand,
    },
    /// Lock commands.
    Lock {
        #[command(subcommand)]
        command: LockCommand,
    },
    /// Conflict commands.
    Conflict {
        #[command(subcommand)]
        command: ConflictCommand,
    },
    /// Workset commands.
    Workset {
        #[command(subcommand)]
        command: WorksetCommand,
    },
    /// Invite commands.
    Invite {
        #[command(subcommand)]
        command: InviteCommand,
    },
    /// Share-link commands.
    Share {
        #[command(subcommand)]
        command: ShareCommand,
    },
    /// Grant commands.
    Grant {
        #[command(subcommand)]
        command: GrantCommand,
    },
    /// Publish commands.
    Publish {
        #[command(subcommand)]
        command: PublishCommand,
    },
    /// Audit log commands.
    Audit {
        #[command(subcommand)]
        command: AuditCommand,
    },
    /// Admin/operator commands. Always gated by the agent-safe mutation policy.
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Auth and credential commands.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Schema introspection for agents and tooling.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Backward-compatible alias for `schema list`.
    Commands,
    ///stdio MCP server exposing the command schema as tools.
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SmokeCommand {
    /// Run the validation smoke suite.
    Run,
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
    /// Start the daemon process. Not yet managed by the CLI.
    Start,
    /// Stop the daemon (daemon.shutdown). Admin-class; requires --yes.
    Stop,
    /// Restart the daemon (daemon.restart). Admin-class; requires --yes.
    Restart,
    /// Show recent daemon logs.
    Logs {
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Subscribe to the daemon event stream (best-effort RPC seam).
    Events,
}

#[derive(Debug, Subcommand)]
enum MountCommand {
    /// Show mount state.
    Status,
    /// Attach a mount.
    Attach {
        #[arg(long)]
        path: String,
    },
    /// Detach a mount. Destructive-class; requires --dry-run or --yes.
    Detach,
    /// List mounts.
    List,
    /// Repair a broken mount. Data-moving; requires --dry-run or --yes.
    Repair,
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
    /// Upload a local file and record/update a metadata file node (server content primitive).
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
    /// Download the current content of a metadata file node (server content primitive).
    Get {
        /// File node ID returned by file put or namespace children.
        #[arg(long, alias = "node-id")]
        node: String,
        /// Local output file path to write; existing paths are not overwritten.
        #[arg(long)]
        out: PathBuf,
    },
    /// File metadata.
    Stat { path: String },
    /// List directory children.
    List {
        #[arg(default_value = "")]
        path: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// File audit history.
    History { path: String },
    /// File versions.
    Versions { path: String },
    /// Restore a file version. Data-moving; requires --dry-run or --yes.
    Restore {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Delete a file to trash. Destructive; requires --dry-run or --yes.
    Delete {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Move/rename a file. Data-moving; requires --dry-run or --yes.
    Move {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Copy a file. Data-moving; requires --dry-run or --yes.
    Copy {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Compute a file checksum.
    Checksum { path: String },
    /// Commit a file write. Low-risk mutation.
    Write {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Read file bytes.
    Read { path: String },
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
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Cache usage and state.
    Status,
    /// List cache entries.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Pin a path. Low-risk mutation.
    Pin {
        #[arg(long)]
        path: String,
    },
    /// Unpin a path. Low-risk mutation.
    Unpin {
        #[arg(long)]
        path: String,
    },
    /// Hydrate a file. Low-risk mutation.
    Hydrate {
        #[arg(long)]
        path: String,
    },
    /// Remove a local copy. Low-risk mutation.
    Dehydrate {
        #[arg(long)]
        path: String,
    },
    /// Evict cache entries. Destructive; requires --dry-run or --yes.
    Evict {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Move the cache location. Data-moving; requires --dry-run or --yes.
    Move {
        #[arg(long)]
        path: String,
    },
    /// Verify cache integrity.
    Verify,
    /// Repair cache state. Data-moving; requires --dry-run or --yes.
    Repair,
}

#[derive(Debug, Subcommand)]
enum TransferCommand {
    /// List transfers.
    List,
    /// Show transfer status.
    Status {
        #[arg(long)]
        transfer_id: String,
    },
    /// Pause a transfer. Low-risk mutation.
    Pause {
        #[arg(long)]
        transfer_id: String,
    },
    /// Resume a transfer. Low-risk mutation.
    Resume {
        #[arg(long)]
        transfer_id: String,
    },
    /// Cancel a transfer. Destructive; requires --dry-run or --yes.
    Cancel {
        #[arg(long)]
        transfer_id: String,
    },
    /// Retry a transfer. Low-risk mutation.
    Retry {
        #[arg(long)]
        transfer_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum SnapshotCommand {
    /// List snapshots.
    List {
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Create a snapshot. Data-moving; requires --dry-run or --yes.
    Create {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Mount a snapshot read-only. Data-moving; requires --dry-run or --yes.
    Mount {
        #[arg(long)]
        snapshot_id: String,
        #[arg(long)]
        path: String,
    },
    /// Unmount a snapshot. Low-risk mutation.
    Unmount {
        #[arg(long)]
        snapshot_id: String,
    },
    /// Diff against a snapshot.
    Diff {
        a: String,
        b: String,
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Restore from a snapshot. Data-moving; requires --dry-run or --yes.
    Restore {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum LockCommand {
    /// List locks.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Acquire a lock. Low-risk mutation.
    Acquire {
        #[arg(long)]
        path: String,
    },
    /// Release a lock. Low-risk mutation.
    Release {
        #[arg(long)]
        path: String,
    },
    /// Show lock status for a path.
    Status {
        #[arg(long)]
        path: String,
    },
    /// Extend a lock. Low-risk mutation.
    Extend {
        #[arg(long)]
        lock_id: String,
        #[arg(long)]
        duration: String,
    },
    /// Break a lock. Admin-class; requires --dry-run or --yes.
    Break {
        #[arg(long)]
        lock_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum ConflictCommand {
    /// List conflicts.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Show a conflict.
    Show {
        #[arg(long)]
        conflict_id: String,
    },
    /// Resolve a conflict. Data-moving; requires --dry-run or --yes.
    Resolve {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Preserve all sides of a conflict. Data-moving; requires --dry-run or --yes.
    PreserveAll {
        #[arg(long)]
        conflict_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum WorksetCommand {
    /// List worksets.
    List,
    /// Show a workset.
    Show {
        #[arg(long)]
        workset_id: String,
    },
    /// Activate a workset. Low-risk mutation.
    Activate {
        #[arg(long)]
        workset_id: String,
    },
    /// Deactivate a workset. Low-risk mutation.
    Deactivate {
        #[arg(long)]
        workset_id: String,
    },
    /// Sync a workset. Data-moving; requires --dry-run or --yes.
    Sync {
        #[arg(long)]
        workset_id: String,
    },
    /// Create a workset. Low-risk mutation.
    Create {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Update a workset. Low-risk mutation.
    Update {
        #[arg(long)]
        workset_id: String,
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum InviteCommand {
    /// Create an invite. Low-risk mutation.
    Create {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// List invites.
    List,
    /// Revoke an invite. Destructive; requires --dry-run or --yes.
    Revoke {
        #[arg(long)]
        invite_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum ShareCommand {
    /// Create a share link. Low-risk mutation.
    Create {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// List shares.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Revoke a share. Destructive; requires --dry-run or --yes.
    Revoke {
        #[arg(long)]
        share_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum GrantCommand {
    /// List grants.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Set a grant. Admin-class; requires --dry-run or --yes.
    Set {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// Revoke a grant. Admin-class; requires --dry-run or --yes.
    Revoke {
        #[arg(long)]
        grant_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum PublishCommand {
    /// Publish a version. Low-risk mutation.
    Create {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
    /// List publishes.
    List {
        #[arg(long)]
        path: Option<String>,
    },
    /// Revoke a publish. Destructive; requires --dry-run or --yes.
    Revoke {
        #[arg(long)]
        publish_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AuditCommand {
    /// Query audit events.
    Events {
        #[arg(long)]
        path: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Show one audit event.
    Event {
        #[arg(long)]
        event_id: String,
    },
    /// Audit by actor.
    Actor {
        #[arg(long)]
        actor_id: String,
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Export the audit log. Data-moving; requires --dry-run or --yes.
    Export {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    /// User administration.
    User {
        #[command(subcommand)]
        command: AdminUserCommand,
    },
    /// Device administration.
    Device {
        #[command(subcommand)]
        command: AdminDeviceCommand,
    },
    /// Token administration.
    Token {
        #[command(subcommand)]
        command: AdminTokenCommand,
    },
    /// Retention policy administration.
    Retention {
        #[command(subcommand)]
        command: AdminRetentionCommand,
    },
    /// Support bundle generation.
    SupportBundle {
        #[command(subcommand)]
        command: AdminSupportBundleCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AdminUserCommand {
    List,
    Show {
        #[arg(long)]
        user_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AdminDeviceCommand {
    List,
    Revoke {
        #[arg(long)]
        device_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AdminTokenCommand {
    Revoke {
        #[arg(long)]
        token_id: String,
    },
}

#[derive(Debug, Subcommand)]
enum AdminRetentionCommand {
    Show,
    Set {
        #[arg(long = "json", value_name = "PAYLOAD")]
        json: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum AdminSupportBundleCommand {
    Create,
}

#[derive(Debug, Subcommand)]
enum AuthCommand {
    /// Local auth/session state (redacted).
    Status,
    /// Enroll a device using a device-or-invite code.
    Enroll {
        #[arg(long)]
        code: String,
    },
    /// Store a login token. The token is read from `BIOHAZARDFS_TOKEN` (env) and
    /// is never accepted via argv, to keep it out of process listings and shell
    /// history. A credential-file/stdin flow may replace the env source later.
    Login,
    /// Clear the local session. Admin-class; requires --yes.
    Logout,
    /// Current actor.
    Whoami,
    /// Credentials file path and rotation.
    Credentials {
        #[command(subcommand)]
        command: AuthCredentialsCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AuthCredentialsCommand {
    /// Show the credentials file path.
    Path,
    /// Rotate local credentials. Low-risk mutation.
    Rotate,
}

#[derive(Debug, Subcommand)]
enum SchemaCommand {
    /// Summarize the implemented command schema (registry derived from known_methods).
    Command { name: String },
    /// List known scaffold commands.
    List,
    /// Describe an event from the event schema.
    Event { name: String },
    /// Describe an error code.
    Error { name: String },
    /// Describe the config schema.
    Config,
    /// Dump commands, events, and errors. Friendly to `--output ndjson`.
    All,
}

#[derive(Debug, Subcommand)]
enum McpCommand {
    /// Serve the MCP stdio surface.
    Serve,
}

fn main() -> ExitCode {
    let mut cli = Cli::parse();
    let command = cli.command.take().unwrap_or(Command::Status);
    if let Command::Mcp {
        command: McpCommand::Serve,
    } = command
    {
        return mcp_serve();
    }
    let (output, code) = run_command(&cli, command);
    println!("{output}");
    ExitCode::from(code)
}

fn run_command(cli: &Cli, command: Command) -> (String, u8) {
    match command {
        Command::Status => client_status_json(cli),
        Command::Version => version_json(cli),
        Command::Doctor { json_deep } => doctor_json(cli, json_deep),
        Command::Smoke {
            command: SmokeCommand::Run,
        } => smoke_run_json(cli),
        Command::Daemon { command } => daemon_json(cli, command),
        Command::Config { command } => config_json(cli, command),
        Command::Mount { command } => mount_json(cli, command),
        Command::Namespace { command } => namespace_json(cli, command),
        Command::Object { command } => object_json(cli, command),
        Command::File { command } => file_json(cli, command),
        Command::Cache { command } => cache_json(cli, command),
        Command::Transfer { command } => transfer_json(cli, command),
        Command::Snapshot { command } => snapshot_json(cli, command),
        Command::Lock { command } => lock_json(cli, command),
        Command::Conflict { command } => conflict_json(cli, command),
        Command::Workset { command } => workset_json(cli, command),
        Command::Invite { command } => invite_json(cli, command),
        Command::Share { command } => share_json(cli, command),
        Command::Grant { command } => grant_json(cli, command),
        Command::Publish { command } => publish_json(cli, command),
        Command::Audit { command } => audit_json(cli, command),
        Command::Admin { command } => admin_json(cli, command),
        Command::Auth { command } => auth_json(cli, command),
        Command::Schema { command } => schema_json(cli, command),
        Command::Commands => schema_list_json(cli),
        // `mcp serve` is handled in main() before run_command; any other mcp shape is a future surface.
        Command::Mcp { .. } => finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "mcp",
                ApiError::new(
                    "method_not_implemented",
                    "mcp subcommands are served over stdio; rerun as `biohazardfs mcp serve`",
                ),
                Source::Cli,
            ),
            EXIT_GENERAL,
        ),
    }
}

// ===========================================================================
// Output rendering
// ===========================================================================

fn finish(cli: &Cli, envelope: CommandResponseEnvelope<Value>, code: u8) -> (String, u8) {
    (render_envelope(cli, envelope), code)
}

/// Render an envelope to the configured output format. This is also where the
/// `--source` and `--request-id` globals are stamped onto the envelope meta so
/// every command path inherits them without each handler repeating the work.
///
/// Operates on `CommandResponseEnvelope<Value>` (not a generic `T`) so the CLI
/// never has to name `serde::Serialize` directly, which keeps `serde` out of
/// this crate's manifest. Concrete data types are converted to `Value` at the
/// handful of construction sites that build typed envelopes.
fn render_envelope(cli: &Cli, mut envelope: CommandResponseEnvelope<Value>) -> String {
    envelope.meta.source = cli_source(cli);
    if let Some(request_id) = &cli.request_id {
        envelope.meta.request_id = request_id.clone();
    }
    match cli.output {
        OutputFormat::Json => serde_json::to_string_pretty(&envelope).expect("envelope serializes"),
        OutputFormat::Ndjson => render_ndjson(envelope),
        OutputFormat::Text => render_text(envelope),
    }
}

fn render_ndjson(envelope: CommandResponseEnvelope<Value>) -> String {
    let streamable = envelope
        .data
        .as_ref()
        .and_then(|data| data.as_object())
        .and_then(|object| {
            NDJSON_LIST_KEYS.iter().find_map(|key| {
                object
                    .get(*key)
                    .and_then(|value| value.as_array())
                    .filter(|array| !array.is_empty() && array.iter().all(|item| item.is_object()))
                    .map(|array| (*key, array.clone()))
            })
        });

    match streamable {
        Some((_, items)) => {
            let mut output = String::new();
            for item in items {
                let mut item_envelope = envelope.clone();
                item_envelope.command = format!("{}.item", envelope.command);
                item_envelope.data = Some(item);
                // Compact JSON, one line per item. `expect` is an invariant: Value always serializes.
                output.push_str(
                    &serde_json::to_string(&item_envelope).expect("ndjson item serializes"),
                );
                output.push('\n');
            }
            output
        }
        None => {
            // Single record on one line. `expect` is an invariant: Value always serializes.
            let mut output = serde_json::to_string(&envelope).expect("envelope serializes");
            output.push('\n');
            output
        }
    }
}

fn render_text(envelope: CommandResponseEnvelope<Value>) -> String {
    if !envelope.ok || envelope.data.is_none() {
        return serde_json::to_string(&envelope).expect("envelope serializes");
    }
    let Some(object) = envelope.data.as_ref().and_then(|data| data.as_object()) else {
        return serde_json::to_string(&envelope).expect("envelope serializes");
    };
    let flat = object
        .values()
        .all(|value| !value.is_array() && !value.is_object());
    if !flat {
        return serde_json::to_string(&envelope).expect("envelope serializes");
    }
    let mut output = format!(
        "{}\t{}\n",
        envelope.command,
        if envelope.ok { "ok" } else { "error" }
    );
    let mut keys: Vec<&String> = object.keys().collect();
    keys.sort();
    for key in keys {
        let value = &object[key];
        let rendered = match value {
            Value::String(string) => string.clone(),
            other => other.to_string(),
        };
        output.push_str(&format!("{key}:\t{rendered}\n"));
    }
    output.trim_end().to_string()
}

fn cli_source(cli: &Cli) -> Source {
    match cli.source {
        Some(SourceArg::Ui) => Source::Ui,
        Some(SourceArg::Cli) => Source::Cli,
        Some(SourceArg::Agent) => Source::Agent,
        Some(SourceArg::Api) => Source::Api,
        Some(SourceArg::Server) => Source::Server,
        Some(SourceArg::Test) => Source::Test,
        None => Source::Cli,
    }
}

// ===========================================================================
// Mutation safety: agent-safe profile gate + dry-run operation tokens.
// ===========================================================================
//
// The default profile is AgentSafe. Under AgentSafe:
//   - Read and LowRisk commands proceed.
//   - Destructive / Admin / DataMoving commands require confirmation:
//       * --dry-run mints a CLI-local token and prints the plan (exit 7).
//       * --yes is accepted but NOT applied: the daemon-issued operation-token
//         flow is not wired yet, so the CLI cannot produce a daemon-valid token
//         and declines to call the daemon (which would otherwise reject with
//         operation_token_required). Returns apply_not_wired (exit 7).
//       * With neither flag, returns confirmation_required (exit 7).
// Classification is read from known_methods::classify(Surface::Cli, method).
// The daemon token-issuance RPC and a real --apply <token> path are planned;
// until they land, destructive daemon mutations are intentionally non-applied.

enum MutationGate {
    Proceed,
    ConfirmationRequired,
    DryRunPlanned,
    /// `--yes` was given for a daemon-gated mutation, but daemon-issued
    /// operation tokens are not wired. Do not call the daemon.
    ApplyPlanned,
}

fn mutation_gate(cli: &Cli, classification: MutationClassification) -> MutationGate {
    match classification {
        MutationClassification::Read | MutationClassification::LowRisk => MutationGate::Proceed,
        MutationClassification::Destructive
        | MutationClassification::Admin
        | MutationClassification::DataMoving => {
            if cli.dry_run {
                MutationGate::DryRunPlanned
            } else if cli.yes {
                MutationGate::ApplyPlanned
            } else {
                MutationGate::ConfirmationRequired
            }
        }
    }
}

fn classification_label(classification: MutationClassification) -> &'static str {
    match classification {
        MutationClassification::Read => "read",
        MutationClassification::LowRisk => "low_risk",
        MutationClassification::Destructive => "destructive",
        MutationClassification::Admin => "admin",
        MutationClassification::DataMoving => "data_moving",
    }
}

fn mutation_gate_description(classification: MutationClassification) -> &'static str {
    match classification {
        MutationClassification::Read | MutationClassification::LowRisk => {
            "no confirmation required"
        }
        MutationClassification::Destructive
        | MutationClassification::Admin
        | MutationClassification::DataMoving => {
            "requires --dry-run (operation token) or --yes under the agent-safe profile"
        }
    }
}

fn confirmation_envelope(
    command: &str,
    classification: MutationClassification,
) -> CommandResponseEnvelope<Value> {
    CommandResponseEnvelope::error(
        command,
        ApiError::with_details(
            "confirmation_required",
            "this command requires --dry-run to mint an operation token or --yes to apply under the agent-safe mutation profile",
            serde_json::json!({
                "policy": "agent_safe",
                "classification": classification_label(classification),
                "required": ["--dry-run", "--yes"],
            }),
        ),
        Source::Cli,
    )
}

/// `--yes` was given for a daemon-gated mutation, but the daemon-issued
/// operation-token flow is not wired. The CLI cannot produce a daemon-valid
/// token, so it declines to call the daemon (which would reject with
/// `operation_token_required`) and surfaces the gap honestly. The daemon
/// token-issuance RPC and a real `--apply <token>` path are planned.
fn apply_planned_envelope(
    command: &str,
    classification: MutationClassification,
) -> CommandResponseEnvelope<Value> {
    CommandResponseEnvelope::error(
        command,
        ApiError::with_details(
            "apply_not_wired",
            "--yes was given, but daemon-issued operation tokens are not yet wired; \
             the CLI cannot apply daemon-gated mutations yet. Use --dry-run to inspect \
             the plan. The daemon token-issuance RPC and --apply <token> are planned.",
            serde_json::json!({
                "policy": "agent_safe",
                "classification": classification_label(classification),
                "planned": ["daemon operation-token RPC", "--apply <token>"],
            }),
        ),
        Source::Cli,
    )
}

fn dry_run_envelope(
    cli: &Cli,
    command: &str,
    method: &str,
    classification: MutationClassification,
    params: &Value,
) -> CommandResponseEnvelope<Value> {
    let token = build_operation_token(cli, method, classification, params);
    let data = serde_json::json!({
        "dry_run": true,
        "method": method,
        "classification": classification_label(classification),
        "operation_token": token.operation_token,
        "params_hash": token.params_hash,
        "plan_hash": token.plan_hash,
        "expires_at": token.expires_at,
        "impact": {
            "files_affected": 0,
            "bytes_affected": 0,
            "server_data_removed": matches!(classification, MutationClassification::Destructive),
            "note": "dry-run plan; backend impact computation lands with daemon/server wiring"
        },
        "apply_hint": "re-run with --yes to apply (per-command --apply <token> lands with the daemon token backend)",
    });
    CommandResponseEnvelope::ok(command, data, Source::Cli)
}

/// Build a real, dependency-free operation token binding method, params hash,
/// plan hash, source, classification, and expiry. Token validation will move
/// server-side once the daemon owns the apply flow; today the CLI mints it.
fn build_operation_token(
    cli: &Cli,
    method: &str,
    classification: MutationClassification,
    params: &Value,
) -> OperationToken {
    let params_hash = sha256_hex(params.to_string().as_bytes());
    let plan_input = format!(
        "{}|{}|{}",
        method,
        classification_label(classification),
        params_hash
    );
    let plan_hash = sha256_hex(plan_input.as_bytes());
    let issued_at = epoch_now_seconds();
    let expires_at = issued_at.saturating_add(DRY_RUN_TOKEN_TTL_SECONDS);
    let source = cli_source(cli);
    let token_input = format!(
        "{}|{}|{}|{}|{}|{}",
        method,
        plan_hash,
        source_label(&source),
        classification_label(classification),
        issued_at,
        expires_at
    );
    let operation_token = format!("op_{}", sha256_hex(token_input.as_bytes()));
    OperationToken {
        operation_token,
        method: method.to_string(),
        params_hash: format!("sha256:{params_hash}"),
        plan_hash: format!("sha256:{plan_hash}"),
        actor_id: None,
        device_id: None,
        source,
        classification,
        expires_at: rfc3339_from_epoch_seconds(expires_at),
    }
}

fn source_label(source: &Source) -> &'static str {
    match source {
        Source::Ui => "ui",
        Source::Cli => "cli",
        Source::Agent => "agent",
        Source::Api => "api",
        Source::Server => "server",
        Source::Test => "test",
    }
}

fn with_pagination(cli: &Cli, params: Value) -> Value {
    let Some(object) = params.as_object() else {
        return params;
    };
    let mut merged = object.clone();
    if let Some(cursor) = &cli.cursor {
        merged.insert("cursor".to_string(), Value::String(cursor.clone()));
    }
    if let Some(fields) = &cli.fields {
        merged.insert("fields".to_string(), Value::String(fields.clone()));
    }
    Value::Object(merged)
}

fn parse_json_payload(
    cli: &Cli,
    command: &'static str,
    payload: &Option<String>,
) -> Result<Value, (String, u8)> {
    let Some(text) = payload else {
        return Ok(serde_json::json!({}));
    };
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(value),
        Err(error) => Err(finish(
            cli,
            CommandResponseEnvelope::error(
                command,
                ApiError::with_details(
                    "invalid_input",
                    format!("--json payload is not valid JSON: {error}"),
                    serde_json::json!({}),
                ),
                Source::Cli,
            ),
            EXIT_USAGE,
        )),
    }
}

// ===========================================================================
// CLI-local commands
// ===========================================================================

fn client_status_json(cli: &Cli) -> (String, u8) {
    let daemon_reachable = local_token().is_some_and(|token| {
        DaemonHttpClient::new(&cli.daemon_endpoint, token)
            .call_status(Source::Cli)
            .is_ok()
    });
    let status = ClientStatus {
        name: "biohazardfs".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        daemon_transport: "dev_loopback_http_json_rpc".to_string(),
        daemon_endpoint: Some(cli.daemon_endpoint.clone()),
        daemon_reachable,
    };
    let envelope = CommandResponseEnvelope::ok(
        "client.status",
        serde_json::to_value(&status).expect("client status serializes"),
        Source::Cli,
    );
    finish(cli, envelope, EXIT_OK)
}

fn version_json(cli: &Cli) -> (String, u8) {
    let data = serde_json::json!({
        "product": "biohazardfs",
        "version": env!("CARGO_PKG_VERSION"),
        "command_schema_version": COMMAND_SCHEMA_VERSION,
        "daemon_schema_version": DAEMON_SCHEMA_VERSION,
        "server_schema_version": SERVER_SCHEMA_VERSION,
        "event_schema_version": EVENT_SCHEMA_VERSION,
    });
    finish(
        cli,
        CommandResponseEnvelope::ok("version", data, Source::Cli),
        EXIT_OK,
    )
}

fn doctor_json(cli: &Cli, json_deep: bool) -> (String, u8) {
    let config_path = resolve_config_file_path(&config_load_options(cli));
    let daemon_reachable = local_token().is_some_and(|token| {
        DaemonHttpClient::new(&cli.daemon_endpoint, token)
            .call_status(Source::Cli)
            .is_ok()
    });
    let checks = vec![
        serde_json::json!({
            "name": "config_file",
            "ok": config_path.exists(),
            "detail": config_path.to_string_lossy(),
        }),
        serde_json::json!({
            "name": "daemon_reachable",
            "ok": daemon_reachable,
            "detail": cli.daemon_endpoint,
        }),
    ];
    let status = if checks.iter().all(|check| {
        check
            .get("ok")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }) {
        "ok"
    } else {
        "degraded"
    };
    let data = serde_json::json!({
        "checks": checks,
        "deep": json_deep,
        "status": status,
        "deep_check_status": if json_deep { "method_not_implemented" } else { "skipped" },
    });
    let mut envelope = CommandResponseEnvelope::ok("doctor", data, Source::Cli);
    if json_deep {
        envelope.warnings.push(Warning {
            code: "doctor_deep_not_implemented".to_string(),
            message:
                "doctor --json-deep is a stub; deep diagnostics land with the diagnostic backend"
                    .to_string(),
        });
    }
    finish(cli, envelope, EXIT_OK)
}

fn smoke_run_json(cli: &Cli) -> (String, u8) {
    let error = ApiError::with_details(
        "method_not_implemented",
        "smoke run is not yet wired; use scripts/ci/*-smoke.sh for validation in this scaffold",
        serde_json::json!({
            "planned_checks": [
                "server_smoke",
                "client_smoke",
                "object_store_smoke",
                "transfer_smoke",
                "db_smoke",
            ],
            "tracking_command": "smoke.run",
        }),
    );
    finish(
        cli,
        CommandResponseEnvelope::error("smoke.run", error, Source::Cli),
        EXIT_GENERAL,
    )
}

// ===========================================================================
// Daemon RPC dispatch (with mutation gate)
// ===========================================================================

fn daemon_rpc_gated(
    cli: &Cli,
    command: &'static str,
    method: &'static str,
    params: Value,
) -> (String, u8) {
    let classification = known_methods::classify(Surface::Cli, method);
    let params = with_pagination(cli, params);
    match mutation_gate(cli, classification) {
        MutationGate::Proceed => daemon_rpc_json(cli, command, method, params),
        MutationGate::ConfirmationRequired => finish(
            cli,
            confirmation_envelope(command, classification),
            EXIT_CONFIRMATION_REQUIRED,
        ),
        MutationGate::DryRunPlanned => finish(
            cli,
            dry_run_envelope(cli, command, method, classification, &params),
            EXIT_CONFIRMATION_REQUIRED,
        ),
        MutationGate::ApplyPlanned => finish(
            cli,
            apply_planned_envelope(command, classification),
            EXIT_CONFIRMATION_REQUIRED,
        ),
    }
}

fn daemon_status_json(cli: &Cli) -> (String, u8) {
    let Some(token) = local_token() else {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "daemon.status",
                ApiError::new(
                    "auth_required",
                    format!("set {LOCAL_TOKEN_ENV} to call the local daemon"),
                ),
                Source::Cli,
            ),
            EXIT_AUTH,
        );
    };

    let client = DaemonHttpClient::new(&cli.daemon_endpoint, token);
    match client.call_status(Source::Cli) {
        Ok(status) => finish(
            cli,
            CommandResponseEnvelope::ok(
                "daemon.status",
                serde_json::to_value(&status).expect("daemon status serializes"),
                Source::Cli,
            ),
            EXIT_OK,
        ),
        Err(error) => {
            let code = daemon_error_code(&error);
            finish(
                cli,
                CommandResponseEnvelope::<Value>::error(
                    "daemon.status",
                    ApiError::new(code, error.to_string()),
                    Source::Cli,
                ),
                daemon_exit_code(code),
            )
        }
    }
}

fn daemon_methods_json(cli: &Cli) -> (String, u8) {
    let Some(token) = local_token() else {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "daemon.methods",
                ApiError::new(
                    "auth_required",
                    format!("set {LOCAL_TOKEN_ENV} to call the local daemon"),
                ),
                Source::Cli,
            ),
            EXIT_AUTH,
        );
    };

    let client = DaemonHttpClient::new(&cli.daemon_endpoint, token);
    let mut request = DaemonRequest::new("daemon.methods", cli_source(cli));
    if let Some(request_id) = &cli.request_id {
        request.id = Some(request_id.clone());
    }
    match client.call::<Value>(&request) {
        Ok(envelope) if envelope.ok => finish(
            cli,
            CommandResponseEnvelope::ok(
                "daemon.methods",
                envelope.data.unwrap_or_else(|| serde_json::json!({})),
                Source::Cli,
            ),
            EXIT_OK,
        ),
        Ok(envelope) => {
            let error = envelope
                .error
                .unwrap_or_else(|| ApiError::new("daemon_error", "daemon returned an error"));
            let normalized_error = if error.code == "unauthorized" {
                ApiError::new("auth_required", "daemon rejected the local auth token")
            } else {
                error
            };
            let exit_code = error_exit_code(&normalized_error.code, EXIT_DAEMON_UNAVAILABLE);
            finish(
                cli,
                CommandResponseEnvelope::<Value>::error(
                    "daemon.methods",
                    normalized_error,
                    Source::Cli,
                ),
                exit_code,
            )
        }
        Err(error) => {
            let code = daemon_error_code(&error);
            finish(
                cli,
                CommandResponseEnvelope::<Value>::error(
                    "daemon.methods",
                    ApiError::new(code, error.to_string()),
                    Source::Cli,
                ),
                daemon_exit_code(code),
            )
        }
    }
}

fn daemon_rpc_json(
    cli: &Cli,
    command: &'static str,
    method: &'static str,
    params: Value,
) -> (String, u8) {
    let Some(token) = local_token() else {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                command,
                ApiError::new(
                    "auth_required",
                    format!("set {LOCAL_TOKEN_ENV} to call the local daemon"),
                ),
                Source::Cli,
            ),
            EXIT_AUTH,
        );
    };

    let client = DaemonHttpClient::new(&cli.daemon_endpoint, token);
    let mut request = DaemonRequest::new(method, cli_source(cli));
    request.params = params;
    if let Some(request_id) = &cli.request_id {
        request.id = Some(request_id.clone());
    }
    match client.call::<Value>(&request) {
        Ok(envelope) if envelope.ok => finish(
            cli,
            CommandResponseEnvelope::ok(
                command,
                envelope.data.unwrap_or_else(|| serde_json::json!({})),
                Source::Cli,
            ),
            EXIT_OK,
        ),
        Ok(envelope) => {
            let error = envelope
                .error
                .unwrap_or_else(|| ApiError::new("daemon_error", "daemon returned an error"));
            let normalized_error = if error.code == "unauthorized" {
                ApiError::new("auth_required", "daemon rejected the local auth token")
            } else {
                error
            };
            let exit_code = if normalized_error.code.ends_with("not_found") {
                EXIT_NOT_FOUND
            } else {
                error_exit_code(&normalized_error.code, EXIT_DAEMON_UNAVAILABLE)
            };
            finish(
                cli,
                CommandResponseEnvelope::<Value>::error(command, normalized_error, Source::Cli),
                exit_code,
            )
        }
        Err(error) => {
            let code = daemon_error_code(&error);
            finish(
                cli,
                CommandResponseEnvelope::<Value>::error(
                    command,
                    ApiError::new(code, error.to_string()),
                    Source::Cli,
                ),
                daemon_exit_code(code),
            )
        }
    }
}

fn daemon_json(cli: &Cli, command: DaemonCommand) -> (String, u8) {
    match command {
        DaemonCommand::Status => daemon_status_json(cli),
        DaemonCommand::Methods => daemon_methods_json(cli),
        DaemonCommand::WorkspaceStatus => daemon_rpc_json(
            cli,
            "daemon.workspace.status",
            "workspace.status",
            serde_json::json!({}),
        ),
        DaemonCommand::WorkspaceList { path } => daemon_rpc_json(
            cli,
            "daemon.workspace.list",
            "workspace.list",
            serde_json::json!({ "path": path }),
        ),
        DaemonCommand::Start => finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "daemon.start",
                ApiError::with_details(
                    "method_not_implemented",
                    "daemon process lifecycle (start) is not managed by the CLI yet",
                    serde_json::json!({"tracking_command": "daemon.start"}),
                ),
                Source::Cli,
            ),
            EXIT_GENERAL,
        ),
        DaemonCommand::Stop => {
            daemon_rpc_gated(cli, "daemon.stop", "daemon.shutdown", serde_json::json!({}))
        }
        DaemonCommand::Restart => daemon_rpc_gated(
            cli,
            "daemon.restart",
            "daemon.restart",
            serde_json::json!({}),
        ),
        DaemonCommand::Logs { limit } => daemon_rpc_json(
            cli,
            "daemon.logs",
            "daemon.logs",
            serde_json::json!({ "limit": limit }),
        ),
        DaemonCommand::Events => daemon_rpc_json(
            cli,
            "daemon.events",
            "daemon.events.subscribe",
            serde_json::json!({}),
        ),
    }
}

fn mount_json(cli: &Cli, command: MountCommand) -> (String, u8) {
    match command {
        MountCommand::Status => {
            daemon_rpc_gated(cli, "mount.status", "mount.status", serde_json::json!({}))
        }
        MountCommand::Attach { path } => daemon_rpc_gated(
            cli,
            "mount.attach",
            "mount.attach",
            serde_json::json!({ "path": path }),
        ),
        MountCommand::Detach => {
            daemon_rpc_gated(cli, "mount.detach", "mount.detach", serde_json::json!({}))
        }
        MountCommand::List => {
            daemon_rpc_gated(cli, "mount.list", "mount.list", serde_json::json!({}))
        }
        MountCommand::Repair => {
            daemon_rpc_gated(cli, "mount.repair", "mount.repair", serde_json::json!({}))
        }
    }
}

fn file_json(cli: &Cli, command: FileCommand) -> (String, u8) {
    match command {
        FileCommand::Put { path, parent, name } => file_put_json(cli, path, parent, name),
        FileCommand::Get { node, out } => file_get_json(cli, node, out),
        FileCommand::Stat { path } => daemon_rpc_gated(
            cli,
            "file.stat",
            "file.stat",
            serde_json::json!({ "path": path }),
        ),
        FileCommand::List { path, limit } => daemon_rpc_gated(
            cli,
            "file.list",
            "file.list",
            serde_json::json!({ "path": path, "limit": limit }),
        ),
        FileCommand::History { path } => daemon_rpc_gated(
            cli,
            "file.history",
            "file.history",
            serde_json::json!({ "path": path }),
        ),
        FileCommand::Versions { path } => daemon_rpc_gated(
            cli,
            "file.versions",
            "file.versions",
            serde_json::json!({ "path": path }),
        ),
        FileCommand::Restore { json } => {
            let params = match parse_json_payload(cli, "file.restore", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "file.restore", "file.restore", params)
        }
        FileCommand::Delete { json } => {
            let params = match parse_json_payload(cli, "file.delete", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "file.delete", "file.delete", params)
        }
        FileCommand::Move { json } => {
            let params = match parse_json_payload(cli, "file.move", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "file.move", "file.move", params)
        }
        FileCommand::Copy { json } => {
            let params = match parse_json_payload(cli, "file.copy", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "file.copy", "file.copy", params)
        }
        FileCommand::Checksum { path } => daemon_rpc_gated(
            cli,
            "file.checksum",
            "file.checksum",
            serde_json::json!({ "path": path }),
        ),
        FileCommand::Write { json } => {
            let params = match parse_json_payload(cli, "file.write", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "file.write", "file.write", params)
        }
        FileCommand::Read { path } => daemon_rpc_gated(
            cli,
            "file.read",
            "file.read",
            serde_json::json!({ "path": path }),
        ),
    }
}

fn cache_json(cli: &Cli, command: CacheCommand) -> (String, u8) {
    match command {
        CacheCommand::Status => {
            daemon_rpc_gated(cli, "cache.status", "cache.status", serde_json::json!({}))
        }
        CacheCommand::List { path } => daemon_rpc_gated(
            cli,
            "cache.list",
            "cache.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        CacheCommand::Pin { path } => daemon_rpc_gated(
            cli,
            "cache.pin",
            "cache.pin",
            serde_json::json!({ "path": path }),
        ),
        CacheCommand::Unpin { path } => daemon_rpc_gated(
            cli,
            "cache.unpin",
            "cache.unpin",
            serde_json::json!({ "path": path }),
        ),
        CacheCommand::Hydrate { path } => daemon_rpc_gated(
            cli,
            "cache.hydrate",
            "cache.hydrate",
            serde_json::json!({ "path": path }),
        ),
        CacheCommand::Dehydrate { path } => daemon_rpc_gated(
            cli,
            "cache.dehydrate",
            "cache.dehydrate",
            serde_json::json!({ "path": path }),
        ),
        CacheCommand::Evict { json } => {
            let params = match parse_json_payload(cli, "cache.evict", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "cache.evict", "cache.evict", params)
        }
        CacheCommand::Move { path } => daemon_rpc_gated(
            cli,
            "cache.move",
            "cache.move",
            serde_json::json!({ "path": path }),
        ),
        CacheCommand::Verify => {
            daemon_rpc_gated(cli, "cache.verify", "cache.verify", serde_json::json!({}))
        }
        CacheCommand::Repair => {
            daemon_rpc_gated(cli, "cache.repair", "cache.repair", serde_json::json!({}))
        }
    }
}

fn transfer_json(cli: &Cli, command: TransferCommand) -> (String, u8) {
    match command {
        TransferCommand::List => {
            daemon_rpc_gated(cli, "transfer.list", "transfer.list", serde_json::json!({}))
        }
        TransferCommand::Status { transfer_id } => daemon_rpc_gated(
            cli,
            "transfer.status",
            "transfer.status",
            serde_json::json!({ "transfer_id": transfer_id }),
        ),
        TransferCommand::Pause { transfer_id } => daemon_rpc_gated(
            cli,
            "transfer.pause",
            "transfer.pause",
            serde_json::json!({ "transfer_id": transfer_id }),
        ),
        TransferCommand::Resume { transfer_id } => daemon_rpc_gated(
            cli,
            "transfer.resume",
            "transfer.resume",
            serde_json::json!({ "transfer_id": transfer_id }),
        ),
        TransferCommand::Cancel { transfer_id } => daemon_rpc_gated(
            cli,
            "transfer.cancel",
            "transfer.cancel",
            serde_json::json!({ "transfer_id": transfer_id }),
        ),
        TransferCommand::Retry { transfer_id } => daemon_rpc_gated(
            cli,
            "transfer.retry",
            "transfer.retry",
            serde_json::json!({ "transfer_id": transfer_id }),
        ),
    }
}

fn snapshot_json(cli: &Cli, command: SnapshotCommand) -> (String, u8) {
    match command {
        SnapshotCommand::List { limit } => daemon_rpc_gated(
            cli,
            "snapshot.list",
            "snapshot.list",
            serde_json::json!({ "limit": limit }),
        ),
        SnapshotCommand::Create { json } => {
            let params = match parse_json_payload(cli, "snapshot.create", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "snapshot.create", "snapshot.create", params)
        }
        SnapshotCommand::Mount { snapshot_id, path } => daemon_rpc_gated(
            cli,
            "snapshot.mount",
            "snapshot.mount",
            serde_json::json!({ "snapshot_id": snapshot_id, "path": path }),
        ),
        SnapshotCommand::Unmount { snapshot_id } => daemon_rpc_gated(
            cli,
            "snapshot.unmount",
            "snapshot.unmount",
            serde_json::json!({ "snapshot_id": snapshot_id }),
        ),
        SnapshotCommand::Diff { a, b, limit } => daemon_rpc_gated(
            cli,
            "snapshot.diff",
            "snapshot.diff",
            serde_json::json!({ "a": a, "b": b, "limit": limit }),
        ),
        SnapshotCommand::Restore { json } => {
            let params = match parse_json_payload(cli, "snapshot.restore", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "snapshot.restore", "snapshot.restore", params)
        }
    }
}

fn lock_json(cli: &Cli, command: LockCommand) -> (String, u8) {
    match command {
        LockCommand::List { path } => daemon_rpc_gated(
            cli,
            "lock.list",
            "lock.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        LockCommand::Acquire { path } => daemon_rpc_gated(
            cli,
            "lock.acquire",
            "lock.acquire",
            serde_json::json!({ "path": path }),
        ),
        LockCommand::Release { path } => daemon_rpc_gated(
            cli,
            "lock.release",
            "lock.release",
            serde_json::json!({ "path": path }),
        ),
        LockCommand::Status { path } => daemon_rpc_gated(
            cli,
            "lock.status",
            "lock.status",
            serde_json::json!({ "path": path }),
        ),
        LockCommand::Extend { lock_id, duration } => daemon_rpc_gated(
            cli,
            "lock.extend",
            "lock.extend",
            serde_json::json!({ "lock_id": lock_id, "duration": duration }),
        ),
        LockCommand::Break { lock_id } => daemon_rpc_gated(
            cli,
            "lock.break",
            "lock.break",
            serde_json::json!({ "lock_id": lock_id }),
        ),
    }
}

fn conflict_json(cli: &Cli, command: ConflictCommand) -> (String, u8) {
    match command {
        ConflictCommand::List { path } => daemon_rpc_gated(
            cli,
            "conflict.list",
            "conflict.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        ConflictCommand::Show { conflict_id } => daemon_rpc_gated(
            cli,
            "conflict.show",
            "conflict.show",
            serde_json::json!({ "conflict_id": conflict_id }),
        ),
        ConflictCommand::Resolve { json } => {
            let params = match parse_json_payload(cli, "conflict.resolve", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "conflict.resolve", "conflict.resolve", params)
        }
        ConflictCommand::PreserveAll { conflict_id } => daemon_rpc_gated(
            cli,
            "conflict.preserve_all",
            "conflict.preserve_all",
            serde_json::json!({ "conflict_id": conflict_id }),
        ),
    }
}

fn workset_json(cli: &Cli, command: WorksetCommand) -> (String, u8) {
    match command {
        WorksetCommand::List => {
            daemon_rpc_gated(cli, "workset.list", "workset.list", serde_json::json!({}))
        }
        WorksetCommand::Show { workset_id } => daemon_rpc_gated(
            cli,
            "workset.show",
            "workset.show",
            serde_json::json!({ "workset_id": workset_id }),
        ),
        WorksetCommand::Activate { workset_id } => daemon_rpc_gated(
            cli,
            "workset.activate",
            "workset.activate",
            serde_json::json!({ "workset_id": workset_id }),
        ),
        WorksetCommand::Deactivate { workset_id } => daemon_rpc_gated(
            cli,
            "workset.deactivate",
            "workset.deactivate",
            serde_json::json!({ "workset_id": workset_id }),
        ),
        WorksetCommand::Sync { workset_id } => daemon_rpc_gated(
            cli,
            "workset.sync",
            "workset.sync",
            serde_json::json!({ "workset_id": workset_id }),
        ),
        WorksetCommand::Create { json } => {
            let params = match parse_json_payload(cli, "workset.create", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "workset.create", "workset.create", params)
        }
        WorksetCommand::Update { workset_id, json } => {
            let params = match parse_json_payload(cli, "workset.update", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            let params = merge_object(params, "workset_id", Value::String(workset_id));
            daemon_rpc_gated(cli, "workset.update", "workset.update", params)
        }
    }
}

fn invite_json(cli: &Cli, command: InviteCommand) -> (String, u8) {
    match command {
        InviteCommand::Create { json } => {
            let params = match parse_json_payload(cli, "invite.create", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "invite.create", "invite.create", params)
        }
        InviteCommand::List => {
            daemon_rpc_gated(cli, "invite.list", "invite.list", serde_json::json!({}))
        }
        InviteCommand::Revoke { invite_id } => daemon_rpc_gated(
            cli,
            "invite.revoke",
            "invite.revoke",
            serde_json::json!({ "invite_id": invite_id }),
        ),
    }
}

fn share_json(cli: &Cli, command: ShareCommand) -> (String, u8) {
    match command {
        ShareCommand::Create { json } => {
            let params = match parse_json_payload(cli, "share.create", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "share.create", "share.create", params)
        }
        ShareCommand::List { path } => daemon_rpc_gated(
            cli,
            "share.list",
            "share.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        ShareCommand::Revoke { share_id } => daemon_rpc_gated(
            cli,
            "share.revoke",
            "share.revoke",
            serde_json::json!({ "share_id": share_id }),
        ),
    }
}

fn grant_json(cli: &Cli, command: GrantCommand) -> (String, u8) {
    match command {
        GrantCommand::List { path } => daemon_rpc_gated(
            cli,
            "grant.list",
            "grant.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        GrantCommand::Set { json } => {
            let params = match parse_json_payload(cli, "grant.set", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "grant.set", "grant.set", params)
        }
        GrantCommand::Revoke { grant_id } => daemon_rpc_gated(
            cli,
            "grant.revoke",
            "grant.revoke",
            serde_json::json!({ "grant_id": grant_id }),
        ),
    }
}

fn publish_json(cli: &Cli, command: PublishCommand) -> (String, u8) {
    match command {
        PublishCommand::Create { json } => {
            let params = match parse_json_payload(cli, "publish.create", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "publish.create", "publish.create", params)
        }
        PublishCommand::List { path } => daemon_rpc_gated(
            cli,
            "publish.list",
            "publish.list",
            serde_json::json!({ "path": path.unwrap_or_default() }),
        ),
        PublishCommand::Revoke { publish_id } => daemon_rpc_gated(
            cli,
            "publish.revoke",
            "publish.revoke",
            serde_json::json!({ "publish_id": publish_id }),
        ),
    }
}

fn audit_json(cli: &Cli, command: AuditCommand) -> (String, u8) {
    match command {
        AuditCommand::Events { path, limit } => daemon_rpc_gated(
            cli,
            "audit.events",
            "audit.events",
            serde_json::json!({ "path": path.unwrap_or_default(), "limit": limit }),
        ),
        AuditCommand::Event { event_id } => daemon_rpc_gated(
            cli,
            "audit.event",
            "audit.event",
            serde_json::json!({ "event_id": event_id }),
        ),
        AuditCommand::Actor { actor_id, limit } => daemon_rpc_gated(
            cli,
            "audit.actor",
            "audit.actor",
            serde_json::json!({ "actor_id": actor_id, "limit": limit }),
        ),
        AuditCommand::Export { json } => {
            let params = match parse_json_payload(cli, "audit.export", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "audit.export", "audit.export", params)
        }
    }
}

fn admin_json(cli: &Cli, command: AdminCommand) -> (String, u8) {
    match command {
        AdminCommand::User {
            command: AdminUserCommand::List,
        } => daemon_rpc_gated(
            cli,
            "admin.user.list",
            "admin.user.list",
            serde_json::json!({}),
        ),
        AdminCommand::User {
            command: AdminUserCommand::Show { user_id },
        } => daemon_rpc_gated(
            cli,
            "admin.user.show",
            "admin.user.show",
            serde_json::json!({ "user_id": user_id }),
        ),
        AdminCommand::Device {
            command: AdminDeviceCommand::List,
        } => daemon_rpc_gated(
            cli,
            "admin.device.list",
            "admin.device.list",
            serde_json::json!({}),
        ),
        AdminCommand::Device {
            command: AdminDeviceCommand::Revoke { device_id },
        } => daemon_rpc_gated(
            cli,
            "admin.device.revoke",
            "admin.device.revoke",
            serde_json::json!({ "device_id": device_id }),
        ),
        AdminCommand::Token {
            command: AdminTokenCommand::Revoke { token_id },
        } => daemon_rpc_gated(
            cli,
            "admin.token.revoke",
            "admin.token.revoke",
            serde_json::json!({ "token_id": token_id }),
        ),
        AdminCommand::Retention {
            command: AdminRetentionCommand::Show,
        } => daemon_rpc_gated(
            cli,
            "admin.retention.show",
            "admin.retention.show",
            serde_json::json!({}),
        ),
        AdminCommand::Retention {
            command: AdminRetentionCommand::Set { json },
        } => {
            let params = match parse_json_payload(cli, "admin.retention.set", &json) {
                Ok(params) => params,
                Err(error) => return error,
            };
            daemon_rpc_gated(cli, "admin.retention.set", "admin.retention.set", params)
        }
        AdminCommand::SupportBundle {
            command: AdminSupportBundleCommand::Create,
        } => daemon_rpc_gated(
            cli,
            "admin.support_bundle.create",
            "admin.support_bundle.create",
            serde_json::json!({}),
        ),
    }
}

fn auth_json(cli: &Cli, command: AuthCommand) -> (String, u8) {
    match command {
        AuthCommand::Status => {
            daemon_rpc_gated(cli, "auth.status", "auth.status", serde_json::json!({}))
        }
        AuthCommand::Enroll { code } => daemon_rpc_gated(
            cli,
            "auth.enroll",
            "auth.enroll",
            serde_json::json!({ "code": code }),
        ),
        AuthCommand::Login => {
            // The login token is never accepted via argv. Read it from the
            // environment; a credential-file/stdin flow can replace this later.
            let token = match std::env::var("BIOHAZARDFS_TOKEN")
                .ok()
                .filter(|value| !value.is_empty())
            {
                Some(value) => value,
                None => {
                    return (
                        serde_json::to_string_pretty(
                            &CommandResponseEnvelope::<serde_json::Value>::error(
                                "auth.login_token",
                                ApiError::new(
                                    "token_required",
                                    "BIOHAZARDFS_TOKEN env var is required; \
                                 the login token is never accepted via argv",
                                ),
                                Source::Cli,
                            ),
                        )
                        .expect("envelope serializes"),
                        EXIT_USAGE,
                    );
                }
            };
            daemon_rpc_gated(
                cli,
                "auth.login",
                "auth.login_token",
                serde_json::json!({ "token": token }),
            )
        }
        AuthCommand::Logout => {
            daemon_rpc_gated(cli, "auth.logout", "auth.logout", serde_json::json!({}))
        }
        AuthCommand::Whoami => {
            daemon_rpc_gated(cli, "auth.whoami", "auth.whoami", serde_json::json!({}))
        }
        AuthCommand::Credentials {
            command: AuthCredentialsCommand::Path,
        } => daemon_rpc_gated(
            cli,
            "auth.credentials.path",
            "auth.credentials_path",
            serde_json::json!({}),
        ),
        AuthCommand::Credentials {
            command: AuthCredentialsCommand::Rotate,
        } => daemon_rpc_gated(
            cli,
            "auth.credentials.rotate",
            "auth.rotate_credentials",
            serde_json::json!({}),
        ),
    }
}

fn merge_object(mut params: Value, key: &str, value: Value) -> Value {
    if let Some(object) = params.as_object_mut() {
        object.insert(key.to_string(), value);
    }
    params
}

// ===========================================================================
// Server-backed namespace/file/object commands (existing)
// ===========================================================================

fn namespace_json(cli: &Cli, command: NamespaceCommand) -> (String, u8) {
    match command {
        NamespaceCommand::Children { parent, limit } => namespace_children_json(cli, parent, limit),
    }
}

fn namespace_children_json(cli: &Cli, parent: Option<String>, limit: u32) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json(cli, "namespace.children", "namespace APIs");
    };

    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json(cli, "namespace.children", error),
    };

    if let Err(error) = validate_namespace_limit(limit) {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error("namespace.children", error, Source::Cli),
            EXIT_USAGE,
        );
    }

    let mut path = format!("/api/v1/namespace/children?limit={limit}");
    if let Some(parent) = parent.as_deref() {
        let parent = match validate_node_id_query_value(parent) {
            Ok(parent) => parent,
            Err(error) => {
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "namespace.children",
                        error,
                        Source::Cli,
                    ),
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
            finish(
                cli,
                CommandResponseEnvelope::ok("namespace.children", data, Source::Cli),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json(cli, "namespace.children", status, payload),
        Err(error) => server_client_error_json(cli, "namespace.children", error),
    }
}

fn file_put_json(
    cli: &Cli,
    path: PathBuf,
    parent: Option<String>,
    name: Option<String>,
) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json(cli, "file.put", "file APIs");
    };
    let file_name = match resolve_file_name(&path, name.as_deref()) {
        Ok(name) => name,
        Err(error) => {
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("file.put", error, Source::Cli),
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
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("file.put", error, Source::Cli),
                EXIT_USAGE,
            );
        }
    };
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json(cli, "file.put", error),
    };
    let content = match read_bounded_input_file(&path) {
        Ok(content) => content,
        Err(error) => {
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("file.put", error, Source::Cli),
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
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "file.put",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not match uploaded file hash and size",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "input_path".to_string(),
                    Value::String(path.to_string_lossy().to_string()),
                );
            }
            finish(
                cli,
                CommandResponseEnvelope::ok("file.put", data, Source::Cli),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json(cli, "file.put", status, payload),
        Err(error) => server_client_error_json(cli, "file.put", error),
    }
}

fn file_get_json(cli: &Cli, node: String, output: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json(cli, "file.get", "file APIs");
    };
    let node = match validate_node_id_query_value(&node) {
        Ok(node) => node.to_string(),
        Err(error) => {
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("file.get", error, Source::Cli),
                EXIT_USAGE,
            );
        }
    };
    if fs::symlink_metadata(&output).is_ok() {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "file.get",
                ApiError::new(
                    "output_exists",
                    "output path already exists; refusing to overwrite without an explicit overwrite command",
                ),
                Source::Cli,
            ),
            EXIT_USAGE,
        );
    }
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json(cli, "file.get", error),
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
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "file.get",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not include content_hex",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            };
            let content = match hex_to_bytes(content_hex) {
                Ok(content) => content,
                Err(error) => {
                    return finish(
                        cli,
                        CommandResponseEnvelope::<Value>::error("file.get", error, Source::Cli),
                        EXIT_SERVER_UNAVAILABLE,
                    );
                }
            };
            let server_hash = data
                .get("content_hash")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            if sha256_hex(&content) != server_hash {
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "file.get",
                        ApiError::new(
                            "content_hash_mismatch",
                            "downloaded file did not match server hash",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Err(error) = write_file_atomically(&output, &content) {
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "file.get",
                        ApiError::new(
                            "file_write_failed",
                            format!("could not write output file: {error}"),
                        ),
                        Source::Cli,
                    ),
                    EXIT_USAGE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.remove("content_hex");
                object.insert(
                    "output_path".to_string(),
                    Value::String(output.to_string_lossy().to_string()),
                );
            }
            finish(
                cli,
                CommandResponseEnvelope::ok("file.get", data, Source::Cli),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json(cli, "file.get", status, payload),
        Err(error) => server_client_error_json(cli, "file.get", error),
    }
}

fn object_json(cli: &Cli, command: ObjectCommand) -> (String, u8) {
    match command {
        ObjectCommand::Put { path } => object_put_json(cli, path),
        ObjectCommand::Get { sha256, out } => object_get_json(cli, sha256, out),
    }
}

fn object_put_json(cli: &Cli, path: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json(cli, "object.put", "content object APIs");
    };
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json(cli, "object.put", error),
    };
    let content = match read_bounded_input_file(&path) {
        Ok(content) => content,
        Err(error) => {
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("object.put", error, Source::Cli),
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
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "object.put",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not match uploaded content hash and size",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.insert(
                    "input_path".to_string(),
                    Value::String(path.to_string_lossy().to_string()),
                );
            }
            finish(
                cli,
                CommandResponseEnvelope::ok("object.put", data, Source::Cli),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json(cli, "object.put", status, payload),
        Err(error) => server_client_error_json(cli, "object.put", error),
    }
}

fn object_get_json(cli: &Cli, sha256: String, output: PathBuf) -> (String, u8) {
    let Some(token) = server_token() else {
        return auth_required_json(cli, "object.get", "content object APIs");
    };
    let sha256 = match validate_content_hash(&sha256) {
        Ok(hash) => hash,
        Err(error) => {
            return finish(
                cli,
                CommandResponseEnvelope::<Value>::error("object.get", error, Source::Cli),
                EXIT_USAGE,
            );
        }
    };
    if fs::symlink_metadata(&output).is_ok() {
        return finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "object.get",
                ApiError::new(
                    "output_exists",
                    "output path already exists; refusing to overwrite without an explicit overwrite command",
                ),
                Source::Cli,
            ),
            EXIT_USAGE,
        );
    }
    let loaded = match load_config(cli) {
        Ok(loaded) => loaded,
        Err(error) => return config_error_json(cli, "object.get", error),
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
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "object.get",
                        ApiError::new(
                            "server_protocol_error",
                            "server response did not include content_hex",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            };
            let content = match hex_to_bytes(content_hex) {
                Ok(content) => content,
                Err(error) => {
                    return finish(
                        cli,
                        CommandResponseEnvelope::<Value>::error("object.get", error, Source::Cli),
                        EXIT_SERVER_UNAVAILABLE,
                    );
                }
            };
            if sha256_hex(&content) != sha256 {
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "object.get",
                        ApiError::new(
                            "content_hash_mismatch",
                            "downloaded content did not match requested hash",
                        ),
                        Source::Cli,
                    ),
                    EXIT_SERVER_UNAVAILABLE,
                );
            }
            if let Err(error) = write_file_atomically(&output, &content) {
                return finish(
                    cli,
                    CommandResponseEnvelope::<Value>::error(
                        "object.get",
                        ApiError::new(
                            "file_write_failed",
                            format!("could not write output file: {error}"),
                        ),
                        Source::Cli,
                    ),
                    EXIT_USAGE,
                );
            }
            if let Some(object) = data.as_object_mut() {
                object.remove("content_hex");
                object.insert(
                    "output_path".to_string(),
                    Value::String(output.to_string_lossy().to_string()),
                );
            }
            finish(
                cli,
                CommandResponseEnvelope::ok("object.get", data, Source::Cli),
                EXIT_OK,
            )
        }
        Ok((status, payload)) => server_error_json(cli, "object.get", status, payload),
        Err(error) => server_client_error_json(cli, "object.get", error),
    }
}

fn auth_required_json(cli: &Cli, command: &'static str, api_name: &str) -> (String, u8) {
    finish(
        cli,
        CommandResponseEnvelope::<Value>::error(
            command,
            ApiError::new(
                "auth_required",
                format!("set {SERVER_TOKEN_ENV} to call BiohazardFS server {api_name}"),
            ),
            Source::Cli,
        ),
        EXIT_AUTH,
    )
}

fn server_error_json(
    cli: &Cli,
    command: &'static str,
    status: u16,
    payload: Value,
) -> (String, u8) {
    let error = payload
        .get("error")
        .cloned()
        .and_then(|error| serde_json::from_value::<ApiError>(error).ok())
        .unwrap_or_else(|| ApiError::new("server_error", "server returned an error"));
    let exit_code = if status == 401
        || status == 403
        || matches!(
            error.code.as_str(),
            "auth_required" | "auth_scope_missing" | "permission_denied" | "device_revoked"
        ) {
        EXIT_AUTH
    } else if status == 404
        || matches!(
            error.code.as_str(),
            "not_found" | "content_object_not_found" | "file_not_found" | "parent_not_found"
        )
        || error.code.ends_with("not_found")
    {
        EXIT_NOT_FOUND
    } else if matches!(
        error.code.as_str(),
        "conflict_detected" | "lock_held" | "lock_required"
    ) {
        EXIT_CONFLICT
    } else if matches!(
        error.code.as_str(),
        "unsupported_platform" | "feature_disabled"
    ) {
        EXIT_UNSUPPORTED_PLATFORM
    } else if matches!(
        error.code.as_str(),
        "confirmation_required" | "operation_token_required" | "operation_token_expired"
    ) {
        EXIT_CONFIRMATION_REQUIRED
    } else if status == 400 || status == 413 || error.code == "invalid_input" {
        EXIT_USAGE
    } else {
        error_exit_code(&error.code, EXIT_SERVER_UNAVAILABLE)
    };
    finish(
        cli,
        CommandResponseEnvelope::<Value>::error(command, error, Source::Cli),
        exit_code,
    )
}

fn server_client_error_json(
    cli: &Cli,
    command: &'static str,
    error: ServerClientError,
) -> (String, u8) {
    let exit_code = if matches!(error.code, "invalid_server_url" | "insecure_server_url") {
        EXIT_USAGE
    } else {
        EXIT_SERVER_UNAVAILABLE
    };
    finish(
        cli,
        CommandResponseEnvelope::<Value>::error(
            command,
            ApiError::new(error.code, error.message),
            Source::Cli,
        ),
        exit_code,
    )
}

// ===========================================================================
// Config commands
// ===========================================================================

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
    let data = serde_json::json!({
        "path": path.to_string_lossy(),
        "exists": path.exists(),
        "profile": profile,
        "schema_version": CONFIG_SCHEMA_VERSION,
    });
    finish(
        cli,
        CommandResponseEnvelope::ok("config.path", data, Source::Cli),
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
                    message: "config show output is redacted by default; pass --redacted to acknowledge this behavior"
                        .to_string(),
                });
            }
            config_ok_json(cli, "config.show", loaded, warnings)
        }
        Err(error) => config_error_json(cli, "config.show", error),
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
                .map(|warning| Warning {
                    code: warning.code,
                    message: warning.message,
                })
                .collect();
            finish(cli, envelope, EXIT_OK)
        }
        Err(error) => config_error_json(cli, "config.validate", error),
    }
}

fn config_ok_json(
    cli: &Cli,
    command: &str,
    loaded: LoadedConfig,
    warnings: Vec<biohazardfs_core::config::ConfigWarning>,
) -> (String, u8) {
    let mut envelope = CommandResponseEnvelope::ok(
        command,
        serde_json::to_value(&loaded).expect("config serializes"),
        Source::Cli,
    );
    envelope.warnings = warnings
        .into_iter()
        .map(|warning| Warning {
            code: warning.code,
            message: warning.message,
        })
        .collect();
    finish(cli, envelope, EXIT_OK)
}

fn config_error_json(cli: &Cli, command: &str, error: ConfigError) -> (String, u8) {
    finish(
        cli,
        CommandResponseEnvelope::<Value>::error(
            command,
            ApiError::new(error.code, error.message),
            Source::Cli,
        ),
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

// ===========================================================================
// Schema introspection commands (registry sourced from known_methods)
// ===========================================================================

fn schema_json(cli: &Cli, command: SchemaCommand) -> (String, u8) {
    match command {
        SchemaCommand::List => schema_list_json(cli),
        SchemaCommand::Command { name } => schema_command_json(cli, name),
        SchemaCommand::Event { name } => schema_event_json(cli, name),
        SchemaCommand::Error { name } => schema_error_json(cli, name),
        SchemaCommand::Config => schema_config_json(cli),
        SchemaCommand::All => schema_all_json(cli),
    }
}

fn schema_list_json(cli: &Cli) -> (String, u8) {
    let commands = known_methods::cli_command_names();
    let count = commands.len();
    let data = serde_json::json!({
        "commands": commands,
        "count": count,
        "note": "scaffold command registry derived from biohazardfs_api_types::known_methods",
        "schema_version": COMMAND_SCHEMA_VERSION,
    });
    finish(
        cli,
        CommandResponseEnvelope::ok("schema.list", data, Source::Cli),
        EXIT_OK,
    )
}

fn schema_command_json(cli: &Cli, name: String) -> (String, u8) {
    match known_methods::find(Surface::Cli, &name) {
        Some(descriptor) => {
            let data = serde_json::json!({
                "name": descriptor.name,
                "group": descriptor.group,
                "surface": "cli",
                "classification": classification_label(descriptor.classification),
                "summary": descriptor.summary,
                "mutation_gate": mutation_gate_description(descriptor.classification),
                "aliases": [],
            });
            finish(
                cli,
                CommandResponseEnvelope::ok("schema.command", data, Source::Cli),
                EXIT_OK,
            )
        }
        None => finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "schema.command",
                ApiError::new("not_found", format!("unknown command: {name}")),
                Source::Cli,
            ),
            EXIT_NOT_FOUND,
        ),
    }
}

fn schema_event_json(cli: &Cli, name: String) -> (String, u8) {
    let events = known_event_names();
    if events.iter().any(|event| *event == name) {
        let data = serde_json::json!({
            "name": name,
            "schema_version": EVENT_SCHEMA_VERSION,
            "summary": format!("event {name}; see DAEMON_API.md event stream contract"),
        });
        finish(
            cli,
            CommandResponseEnvelope::ok("schema.event", data, Source::Cli),
            EXIT_OK,
        )
    } else {
        finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "schema.event",
                ApiError::with_details(
                    "not_found",
                    format!("unknown event: {name}"),
                    serde_json::json!({ "known_events": events }),
                ),
                Source::Cli,
            ),
            EXIT_NOT_FOUND,
        )
    }
}

fn schema_error_json(cli: &Cli, name: String) -> (String, u8) {
    let codes = known_error_codes();
    if codes.iter().any(|code| *code == name) {
        let data = serde_json::json!({
            "code": name,
            "schema_version": COMMAND_SCHEMA_VERSION,
            "summary": format!("error code {name}; see COMMANDS.md error codes"),
        });
        finish(
            cli,
            CommandResponseEnvelope::ok("schema.error", data, Source::Cli),
            EXIT_OK,
        )
    } else {
        finish(
            cli,
            CommandResponseEnvelope::<Value>::error(
                "schema.error",
                ApiError::with_details(
                    "not_found",
                    format!("unknown error code: {name}"),
                    serde_json::json!({ "known_error_codes": codes }),
                ),
                Source::Cli,
            ),
            EXIT_NOT_FOUND,
        )
    }
}

fn schema_config_json(cli: &Cli) -> (String, u8) {
    let data = serde_json::json!({
        "schema_version": COMMAND_SCHEMA_VERSION,
        "format": "toml",
        "keys": [
            "profile", "server_url", "mount.name", "mount.path",
            "cache.path", "cache.limit_bytes", "mutation_policy",
            "output.default", "credentials.path", "features.*",
        ],
        "note": "config schema scaffold; full JSON schema lands with config doctor work",
    });
    finish(
        cli,
        CommandResponseEnvelope::ok("schema.config", data, Source::Cli),
        EXIT_OK,
    )
}

fn schema_all_json(cli: &Cli) -> (String, u8) {
    let commands: Vec<Value> = known_methods::cli_commands()
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "name": descriptor.name,
                "group": descriptor.group,
                "classification": classification_label(descriptor.classification),
                "summary": descriptor.summary,
            })
        })
        .collect();
    let data = serde_json::json!({
        "commands": commands,
        "events": known_event_names(),
        "errors": known_error_codes(),
        "schema_version": COMMAND_SCHEMA_VERSION,
    });
    finish(
        cli,
        CommandResponseEnvelope::ok("schema.all", data, Source::Cli),
        EXIT_OK,
    )
}

fn known_event_names() -> Vec<&'static str> {
    use biohazardfs_api_types::event_types as events;
    vec![
        events::DAEMON_STARTED,
        events::DAEMON_STOPPING,
        events::DAEMON_HEALTH_CHANGED,
        events::AUTH_CHANGED,
        events::MOUNT_ATTACHED,
        events::MOUNT_DETACHED,
        events::MOUNT_HEALTH_CHANGED,
        events::FILE_CHANGED,
        events::CACHE_STATE_CHANGED,
        events::CACHE_QUOTA_WARNING,
        events::TRANSFER_QUEUED,
        events::TRANSFER_PROGRESS,
        events::TRANSFER_COMPLETED,
        events::TRANSFER_FAILED,
        events::LOCK_CHANGED,
        events::CONFLICT_DETECTED,
        events::CONFLICT_RESOLVED,
        events::SNAPSHOT_CREATED,
        events::SNAPSHOT_MOUNTED,
        events::AUDIT_EVENT_RECORDED,
        events::WARNING_RAISED,
    ]
}

fn known_error_codes() -> Vec<&'static str> {
    vec![
        "invalid_input",
        "schema_validation_failed",
        "confirmation_required",
        "operation_token_required",
        "operation_token_expired",
        "permission_denied",
        "auth_required",
        "device_revoked",
        "not_found",
        "conflict_detected",
        "lock_required",
        "lock_held",
        "network_unavailable",
        "server_unavailable",
        "cache_full",
        "cache_corrupt",
        "transfer_failed",
        "mount_unavailable",
        "unsupported_platform",
        "feature_disabled",
        "internal_error",
        "method_not_implemented",
    ]
}

// ===========================================================================
// MCP stdio surface
// ===========================================================================
//
// Minimal JSON-RPC 2.0 stdio seam. Reads newline-delimited requests, answers
// `initialize`, `ping`, and `tools/list` (tools generated from known_methods),
// and returns typed JSON-RPC errors for unknown methods or tool calls. Tool
// execution itself routes through the CLI command tree and is not wired here.

fn mcp_serve() -> ExitCode {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        match line {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => {
                let response = handle_mcp_request(&line);
                // Write failures on stdout mean the peer is gone; stop serving.
                if writeln!(out, "{response}").is_err() {
                    break;
                }
                let _ = out.flush();
            }
            Err(_) => break,
        }
    }
    ExitCode::from(EXIT_OK)
}

fn handle_mcp_request(line: &str) -> String {
    let parsed: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(error) => {
            return mcp_error(None, -32700, &format!("parse error: {error}"), Value::Null);
        }
    };
    let id = parsed.get("id").cloned();
    let method = parsed
        .get("method")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    match method {
        "initialize" => mcp_result(id, mcp_initialize()),
        "ping" => mcp_result(id, serde_json::json!({})),
        "tools/list" => mcp_result(id, mcp_tools_list()),
        "tools/call" => mcp_tools_call(id, &parsed),
        other => mcp_error(id, -32601, &format!("unknown method: {other}"), Value::Null),
    }
}

fn mcp_initialize() -> Value {
    serde_json::json!({
        "protocolVersion": MCP_PROTOCOL_VERSION,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {
            "name": "biohazardfs",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

fn mcp_tools_list() -> Value {
    let tools: Vec<Value> = known_methods::cli_commands()
        .iter()
        .map(|descriptor| {
            serde_json::json!({
                "name": descriptor.name,
                "description": descriptor.summary,
                "annotations": {
                    "group": descriptor.group,
                    "classification": classification_label(descriptor.classification),
                    "surface": "cli",
                }
            })
        })
        .collect();
    serde_json::json!({"tools": tools})
}

fn mcp_tools_call(id: Option<Value>, request: &Value) -> String {
    let name = request
        .get("params")
        .and_then(|params| params.get("name"))
        .and_then(|name| name.as_str())
        .unwrap_or("");
    if known_methods::find(Surface::Cli, name).is_none() {
        return mcp_error(
            id,
            -32602,
            &format!("unknown tool: {name}"),
            serde_json::json!({"tool": name}),
        );
    }
    // Tool execution routes through the CLI command tree; not wired in this seam.
    mcp_error(
        id,
        -32603,
        &format!(
            "tool {name} is not executable via MCP in this build; run the matching biohazardfs CLI subcommand"
        ),
        serde_json::json!({"tool": name, "code": "method_not_implemented"}),
    )
}

fn mcp_result(id: Option<Value>, result: Value) -> String {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "result": result,
    });
    serde_json::to_string(&response).expect("mcp result serializes")
}

fn mcp_error(id: Option<Value>, code: i64, message: &str, data: Value) -> String {
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {"code": code, "message": message, "data": data},
    });
    serde_json::to_string(&response).expect("mcp error serializes")
}

// ===========================================================================
// Error/exit code mapping helpers
// ===========================================================================

/// Map an ApiError code to a shell exit code for codes that have a dedicated
/// mapping independent of HTTP status. Returns `default` for unmapped codes.
fn error_exit_code(code: &str, default: u8) -> u8 {
    match code {
        "auth_required" | "permission_denied" | "device_revoked" => EXIT_AUTH,
        "conflict_detected" | "lock_held" | "lock_required" => EXIT_CONFLICT,
        "unsupported_platform" | "feature_disabled" => EXIT_UNSUPPORTED_PLATFORM,
        "confirmation_required" | "operation_token_required" | "operation_token_expired" => {
            EXIT_CONFIRMATION_REQUIRED
        }
        "method_not_implemented" | "internal_error" => EXIT_GENERAL,
        _ => default,
    }
}

fn daemon_exit_code(code: &str) -> u8 {
    match code {
        "auth_required" => EXIT_AUTH,
        "invalid_daemon_endpoint" => EXIT_USAGE,
        _ => error_exit_code(code, EXIT_DAEMON_UNAVAILABLE),
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

// ===========================================================================
// Validation + low-level helpers
// ===========================================================================

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
) -> Result<(u16, Value), ServerClientError> {
    server_request_json("GET", server_url, path, bearer_token, &[])
}

fn server_request_json(
    method: &'static str,
    server_url: &str,
    path: &str,
    bearer_token: Option<&str>,
    body: &[u8],
) -> Result<(u16, Value), ServerClientError> {
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
    let payload = serde_json::from_str::<Value>(body).map_err(|error| ServerClientError {
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

// ===========================================================================
// Time helpers (no `time` crate dependency; CLI must stay dependency-light).
// ===========================================================================

fn epoch_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/// Format a UTC epoch-seconds value as RFC3339 (`YYYY-MM-DDTHH:MM:SSZ`) using
/// Howard Hinnant's civil_from_days algorithm. Avoids pulling in a datetime
/// crate just for operation-token expiry stamps.
fn rfc3339_from_epoch_seconds(seconds: u64) -> String {
    let days = (seconds / 86400) as i64;
    let secs_of_day = seconds % 86400;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
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

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("cli parses")
    }

    // ----- existing tests preserved -----

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

    // ----- global flag parsing -----

    #[test]
    fn parses_all_seven_global_flags() {
        let cli = parse(&[
            "biohazardfs",
            "--output",
            "ndjson",
            "--fields",
            "id,name",
            "--cursor",
            "cur_abc",
            "--source",
            "agent",
            "--request-id",
            "req_test_1",
            "--dry-run",
            "--yes",
            "version",
        ]);
        assert_eq!(cli.output, OutputFormat::Ndjson);
        assert_eq!(cli.fields.as_deref(), Some("id,name"));
        assert_eq!(cli.cursor.as_deref(), Some("cur_abc"));
        assert_eq!(cli.source, Some(SourceArg::Agent));
        assert_eq!(cli.request_id.as_deref(), Some("req_test_1"));
        assert!(cli.dry_run);
        assert!(cli.yes);
        assert!(matches!(cli.command, Some(Command::Version)));
    }

    #[test]
    fn output_format_defaults_to_json() {
        let cli = parse(&["biohazardfs", "status"]);
        assert_eq!(cli.output, OutputFormat::Json);
    }

    #[test]
    fn source_defaults_to_cli_when_omitted() {
        let cli = parse(&["biohazardfs", "version"]);
        assert_eq!(cli.source, None);
        assert_eq!(cli_source(&cli), Source::Cli);
    }

    #[test]
    fn rejects_unknown_output_format() {
        let result = Cli::try_parse_from(["biohazardfs", "--output", "yaml", "version"]);
        assert!(result.is_err(), "yaml is not a supported output format");
    }

    #[test]
    fn rejects_unknown_source_label() {
        let result = Cli::try_parse_from(["biohazardfs", "--source", "daemon", "version"]);
        assert!(result.is_err(), "daemon is not a valid source label");
    }

    // ----- new subcommand parsing -----

    #[test]
    fn parses_cache_evict_subcommand() {
        let cli = parse(&[
            "biohazardfs",
            "cache",
            "evict",
            "--json",
            "{\"older_than\":\"30d\"}",
        ]);
        let Some(Command::Cache {
            command: CacheCommand::Evict { json },
        }) = cli.command
        else {
            panic!("expected cache evict");
        };
        assert_eq!(json.as_deref(), Some("{\"older_than\":\"30d\"}"));
    }

    #[test]
    fn parses_file_delete_subcommand() {
        let cli = parse(&[
            "biohazardfs",
            "file",
            "delete",
            "--json",
            "{\"path\":\"/p\"}",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::File {
                command: FileCommand::Delete { .. }
            })
        ));
    }

    #[test]
    fn parses_snapshot_create_subcommand() {
        let cli = parse(&[
            "biohazardfs",
            "snapshot",
            "create",
            "--json",
            "{\"name\":\"before-review\"}",
        ]);
        assert!(matches!(
            cli.command,
            Some(Command::Snapshot {
                command: SnapshotCommand::Create { .. }
            })
        ));
    }

    #[test]
    fn parses_admin_user_show_subcommand() {
        let cli = parse(&["biohazardfs", "admin", "user", "show", "--user-id", "usr_1"]);
        let Some(Command::Admin {
            command:
                AdminCommand::User {
                    command: AdminUserCommand::Show { user_id },
                },
        }) = cli.command
        else {
            panic!("expected admin user show");
        };
        assert_eq!(user_id, "usr_1");
    }

    #[test]
    fn parses_admin_support_bundle_create() {
        let cli = parse(&["biohazardfs", "admin", "support-bundle", "create"]);
        assert!(matches!(
            cli.command,
            Some(Command::Admin {
                command: AdminCommand::SupportBundle {
                    command: AdminSupportBundleCommand::Create
                }
            })
        ));
    }

    #[test]
    fn parses_auth_credentials_rotate() {
        let cli = parse(&["biohazardfs", "auth", "credentials", "rotate"]);
        assert!(matches!(
            cli.command,
            Some(Command::Auth {
                command: AuthCommand::Credentials {
                    command: AuthCredentialsCommand::Rotate
                }
            })
        ));
    }

    #[test]
    fn parses_mcp_serve() {
        let cli = parse(&["biohazardfs", "mcp", "serve"]);
        assert!(matches!(
            cli.command,
            Some(Command::Mcp {
                command: McpCommand::Serve
            })
        ));
    }

    #[test]
    fn parses_smoke_run_and_version_and_doctor() {
        let cli = parse(&["biohazardfs", "smoke", "run"]);
        assert!(matches!(
            cli.command,
            Some(Command::Smoke {
                command: SmokeCommand::Run
            })
        ));
        let cli = parse(&["biohazardfs", "version"]);
        assert!(matches!(cli.command, Some(Command::Version)));
        let cli = parse(&["biohazardfs", "doctor", "--json-deep"]);
        let Some(Command::Doctor { json_deep }) = cli.command else {
            panic!("expected doctor");
        };
        assert!(json_deep);
    }

    #[test]
    fn parses_lock_extend_with_duration() {
        let cli = parse(&[
            "biohazardfs",
            "lock",
            "extend",
            "--lock-id",
            "lck_1",
            "--duration",
            "2h",
        ]);
        let Some(Command::Lock {
            command: LockCommand::Extend { lock_id, duration },
        }) = cli.command
        else {
            panic!("expected lock extend");
        };
        assert_eq!(lock_id, "lck_1");
        assert_eq!(duration, "2h");
    }

    // ----- mutation gate / exit codes -----

    #[test]
    fn mutation_gate_proceeds_for_read_and_low_risk() {
        let cli = parse(&["biohazardfs", "cache", "status"]);
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::Read),
            MutationGate::Proceed
        ));
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::LowRisk),
            MutationGate::Proceed
        ));
    }

    #[test]
    fn mutation_gate_blocks_destructive_without_flags() {
        let cli = parse(&["biohazardfs", "cache", "evict"]);
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::Destructive),
            MutationGate::ConfirmationRequired
        ));
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::Admin),
            MutationGate::ConfirmationRequired
        ));
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::DataMoving),
            MutationGate::ConfirmationRequired
        ));
    }

    #[test]
    fn mutation_gate_dry_run_plans_destructive() {
        let cli = parse(&["biohazardfs", "--dry-run", "cache", "evict"]);
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::Destructive),
            MutationGate::DryRunPlanned
        ));
    }

    #[test]
    fn mutation_gate_yes_does_not_apply_destructive() {
        // --yes on a daemon-gated destructive mutation does NOT proceed: the
        // daemon-issued operation-token flow is not wired, so the CLI declines
        // to call the daemon (which would otherwise reject with
        // operation_token_required) and returns ApplyPlanned instead.
        let cli = parse(&["biohazardfs", "--yes", "cache", "evict"]);
        assert!(matches!(
            mutation_gate(&cli, MutationClassification::Destructive),
            MutationGate::ApplyPlanned
        ));
    }

    #[test]
    fn destructive_command_with_yes_returns_apply_not_wired() {
        let cli = parse(&["biohazardfs", "--yes", "cache", "evict"]);
        let (output, code) =
            daemon_rpc_gated(&cli, "cache.evict", "cache.evict", serde_json::json!({}));
        assert_eq!(code, EXIT_CONFIRMATION_REQUIRED);
        assert!(output.contains("apply_not_wired"));
        assert!(output.contains("agent_safe"));
    }

    #[test]
    fn destructive_command_returns_confirmation_required_exit_code() {
        let cli = parse(&["biohazardfs", "cache", "evict"]);
        let (output, code) =
            daemon_rpc_gated(&cli, "cache.evict", "cache.evict", serde_json::json!({}));
        assert_eq!(code, EXIT_CONFIRMATION_REQUIRED);
        assert!(output.contains("confirmation_required"));
        assert!(output.contains("agent_safe"));
    }

    #[test]
    fn destructive_command_with_dry_run_returns_token_and_exit_seven() {
        let cli = parse(&["biohazardfs", "--dry-run", "cache", "evict"]);
        let (output, code) = daemon_rpc_gated(
            &cli,
            "cache.evict",
            "cache.evict",
            serde_json::json!({ "older_than": "30d" }),
        );
        assert_eq!(code, EXIT_CONFIRMATION_REQUIRED);
        let value: Value = serde_json::from_str(&output).expect("dry-run output is json");
        assert_eq!(value["ok"], true);
        assert_eq!(value["data"]["dry_run"], true);
        let token = value["data"]["operation_token"]
            .as_str()
            .expect("operation_token present");
        assert!(token.starts_with("op_"));
        assert!(value["data"].get("plan_hash").is_some());
        assert!(value["data"].get("params_hash").is_some());
        assert!(value["data"].get("expires_at").is_some());
    }

    #[test]
    fn read_command_is_not_blocked_by_gate_and_reaches_auth_check() {
        // cache.status is Read; gate Proceeds and daemon_rpc_json needs a local token.
        // Without a token we land on auth_required (exit 3), proving the gate did not short-circuit.
        let cli = parse(&["biohazardfs", "cache", "status"]);
        let (output, code) =
            daemon_rpc_gated(&cli, "cache.status", "cache.status", serde_json::json!({}));
        assert_eq!(code, EXIT_AUTH);
        assert!(output.contains("auth_required"));
    }

    #[test]
    fn dry_run_token_is_deterministic_and_hashes_params() {
        let cli = parse(&[
            "biohazardfs",
            "--dry-run",
            "--source",
            "agent",
            "file",
            "delete",
        ]);
        let params_a = serde_json::json!({"path": "/a"});
        let params_b = serde_json::json!({"path": "/a"});
        let params_c = serde_json::json!({"path": "/b"});
        let token_a = build_operation_token(
            &cli,
            "file.delete",
            MutationClassification::Destructive,
            &params_a,
        );
        let token_b = build_operation_token(
            &cli,
            "file.delete",
            MutationClassification::Destructive,
            &params_b,
        );
        let token_c = build_operation_token(
            &cli,
            "file.delete",
            MutationClassification::Destructive,
            &params_c,
        );
        assert_eq!(token_a.operation_token, token_b.operation_token);
        assert_ne!(token_a.operation_token, token_c.operation_token);
        assert_ne!(token_a.params_hash, token_c.params_hash);
        assert!(token_a.operation_token.starts_with("op_"));
        assert!(token_a.params_hash.starts_with("sha256:"));
        assert!(token_a.plan_hash.starts_with("sha256:"));
    }

    #[test]
    fn confirmation_envelope_carries_required_flags() {
        let env = confirmation_envelope("file.delete", MutationClassification::Destructive);
        let value = serde_json::to_value(&env).expect("envelope serializes");
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], "confirmation_required");
        assert_eq!(value["error"]["details"]["policy"], "agent_safe");
        assert_eq!(value["error"]["details"]["classification"], "destructive");
    }

    #[test]
    fn error_exit_code_maps_conflict_and_unsupported() {
        assert_eq!(
            error_exit_code("conflict_detected", EXIT_GENERAL),
            EXIT_CONFLICT
        );
        assert_eq!(error_exit_code("lock_held", EXIT_GENERAL), EXIT_CONFLICT);
        assert_eq!(
            error_exit_code("unsupported_platform", EXIT_GENERAL),
            EXIT_UNSUPPORTED_PLATFORM
        );
        assert_eq!(
            error_exit_code("feature_disabled", EXIT_GENERAL),
            EXIT_UNSUPPORTED_PLATFORM
        );
        assert_eq!(
            error_exit_code("method_not_implemented", EXIT_GENERAL),
            EXIT_GENERAL
        );
        assert_eq!(
            error_exit_code("unknown_code", EXIT_DAEMON_UNAVAILABLE),
            EXIT_DAEMON_UNAVAILABLE
        );
    }

    // ----- schema registry -----

    #[test]
    fn schema_list_derives_from_known_methods() {
        let cli = parse(&["biohazardfs", "schema", "list"]);
        let (output, code) = schema_list_json(&cli);
        assert_eq!(code, EXIT_OK);
        let value: Value = serde_json::from_str(&output).expect("schema list is json");
        assert_eq!(value["command"].as_str(), Some("schema.list"));
        let commands = value["data"]["commands"]
            .as_array()
            .expect("commands is an array");
        // CLI-only commands from known_methods must appear.
        assert!(commands.iter().any(|entry| entry == "client.status"));
        assert!(commands.iter().any(|entry| entry == "doctor"));
        assert!(commands.iter().any(|entry| entry == "mcp.serve"));
        // Mirrored daemon commands must appear.
        assert!(commands.iter().any(|entry| entry == "cache.evict"));
        assert!(commands.iter().any(|entry| entry == "file.delete"));
    }

    #[test]
    fn schema_command_describes_known_command() {
        let cli = parse(&["biohazardfs", "schema", "command", "file.delete"]);
        let (output, code) = schema_command_json(&cli, "file.delete".to_string());
        assert_eq!(code, EXIT_OK);
        let value: Value = serde_json::from_str(&output).expect("schema command is json");
        assert_eq!(value["data"]["name"].as_str(), Some("file.delete"));
        assert_eq!(
            value["data"]["classification"].as_str(),
            Some("destructive")
        );
        assert!(value["data"].get("mutation_gate").is_some());
    }

    #[test]
    fn schema_command_unknown_returns_not_found() {
        let cli = parse(&["biohazardfs", "schema", "command", "nope.nope"]);
        let (output, code) = schema_command_json(&cli, "nope.nope".to_string());
        assert_eq!(code, EXIT_NOT_FOUND);
        assert!(output.contains("not_found"));
    }

    #[test]
    fn schema_error_known_and_unknown() {
        let cli = parse(&["biohazardfs", "schema", "error", "confirmation_required"]);
        let (output, code) = schema_error_json(&cli, "confirmation_required".to_string());
        assert_eq!(code, EXIT_OK);
        assert!(output.contains("confirmation_required"));
        let (output, code) = schema_error_json(&cli, "totally_made_up".to_string());
        assert_eq!(code, EXIT_NOT_FOUND);
        assert!(output.contains("unknown error code"));
    }

    #[test]
    fn commands_alias_matches_schema_list() {
        let cli = parse(&["biohazardfs", "commands"]);
        let (output, _) = schema_list_json(&cli);
        let value: Value = serde_json::from_str(&output).expect("schema list is json");
        assert_eq!(value["command"].as_str(), Some("schema.list"));
    }

    // ----- envelope construction + globals -----

    #[test]
    fn version_envelope_carries_source_and_request_id_globals() {
        let cli = parse(&[
            "biohazardfs",
            "--source",
            "agent",
            "--request-id",
            "req_test_global",
            "version",
        ]);
        let (output, code) = version_json(&cli);
        assert_eq!(code, EXIT_OK);
        let value: Value = serde_json::from_str(&output).expect("version output is json");
        assert_eq!(value["command"].as_str(), Some("version"));
        assert_eq!(value["meta"]["source"].as_str(), Some("agent"));
        assert_eq!(
            value["meta"]["request_id"].as_str(),
            Some("req_test_global")
        );
    }

    #[test]
    fn version_envelope_defaults_to_cli_source() {
        let cli = parse(&["biohazardfs", "version"]);
        let (output, _) = version_json(&cli);
        let value: Value = serde_json::from_str(&output).expect("version output is json");
        assert_eq!(value["meta"]["source"].as_str(), Some("cli"));
    }

    #[test]
    fn smoke_run_returns_method_not_implemented() {
        let cli = parse(&["biohazardfs", "smoke", "run"]);
        let (output, code) = smoke_run_json(&cli);
        assert_eq!(code, EXIT_GENERAL);
        assert!(output.contains("method_not_implemented"));
        assert!(output.contains("smoke.run"));
    }

    #[test]
    fn pagination_globals_merge_into_list_params() {
        let cli = parse(&[
            "biohazardfs",
            "--cursor",
            "cur_xyz",
            "--fields",
            "id,name",
            "transfer",
            "list",
        ]);
        let merged = with_pagination(&cli, serde_json::json!({}));
        assert_eq!(merged["cursor"], "cur_xyz");
        assert_eq!(merged["fields"], "id,name");
    }

    // ----- output renderers -----

    #[test]
    fn ndjson_streams_list_items_as_envelopes() {
        let cli = parse(&["biohazardfs", "--output", "ndjson", "transfer", "list"]);
        let envelope = CommandResponseEnvelope::ok(
            "transfer.list",
            serde_json::json!({
                "transfers": [
                    {"id": "t1", "status": "queued"},
                    {"id": "t2", "status": "completed"},
                ],
                "next_cursor": "cur_next",
            }),
            Source::Cli,
        );
        let rendered = render_envelope(&cli, envelope);
        let lines: Vec<&str> = rendered.trim_end().lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"t1\""));
        assert!(lines[1].contains("\"t2\""));
        assert!(lines[0].contains("transfer.list.item"));
    }

    #[test]
    fn ndjson_falls_back_to_single_line_for_non_list() {
        let cli = parse(&["biohazardfs", "--output", "ndjson", "version"]);
        let envelope = CommandResponseEnvelope::ok(
            "version",
            serde_json::json!({"version": "0.1.0"}),
            Source::Cli,
        );
        let rendered = render_envelope(&cli, envelope);
        let lines: Vec<&str> = rendered.trim_end().lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn text_renders_flat_object_as_key_value() {
        let cli = parse(&["biohazardfs", "--output", "text", "version"]);
        let envelope = CommandResponseEnvelope::ok(
            "version",
            serde_json::json!({"version": "0.1.0", "product": "biohazardfs"}),
            Source::Cli,
        );
        let rendered = render_envelope(&cli, envelope);
        assert!(rendered.starts_with("version\tok"));
        assert!(rendered.contains("product:\tbiohazardfs"));
        assert!(rendered.contains("version:\t0.1.0"));
    }

    #[test]
    fn text_falls_back_to_json_for_nested_data() {
        let cli = parse(&["biohazardfs", "--output", "text", "transfer", "list"]);
        let envelope = CommandResponseEnvelope::ok(
            "transfer.list",
            serde_json::json!({"transfers": [{"id": "t1"}]}),
            Source::Cli,
        );
        let rendered = render_envelope(&cli, envelope);
        // Nested object/array -> compact JSON fallback (single line, no tabs header).
        assert!(rendered.contains("\"transfers\""));
        assert!(!rendered.contains('\t'));
    }

    // ----- MCP seam -----

    #[test]
    fn mcp_initialize_responds_with_server_info() {
        let response = handle_mcp_request(r#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#);
        assert!(response.contains("\"serverInfo\""));
        assert!(response.contains("biohazardfs"));
        assert!(response.contains(MCP_PROTOCOL_VERSION));
    }

    #[test]
    fn mcp_tools_list_exposes_known_methods() {
        let response = handle_mcp_request(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#);
        assert!(response.contains("\"tools\""));
        assert!(response.contains("cache.pin"));
        assert!(response.contains("file.delete"));
    }

    #[test]
    fn mcp_ping_returns_empty_result() {
        let response = handle_mcp_request(r#"{"jsonrpc":"2.0","id":3,"method":"ping"}"#);
        assert!(response.contains("\"result\""));
    }

    #[test]
    fn mcp_unknown_method_returns_method_not_found() {
        let response = handle_mcp_request(r#"{"jsonrpc":"2.0","id":4,"method":"bogus"}"#);
        assert!(response.contains("-32601"));
    }

    #[test]
    fn mcp_tools_call_returns_method_not_implemented_for_known_tool() {
        let request =
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"cache.status"}}"#;
        let response = handle_mcp_request(request);
        assert!(response.contains("method_not_implemented"));
        assert!(response.contains("-32603"));
    }

    #[test]
    fn mcp_tools_call_rejects_unknown_tool() {
        let request =
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope.tool"}}"#;
        let response = handle_mcp_request(request);
        assert!(response.contains("-32602"));
    }

    #[test]
    fn mcp_parse_error_returns_minus_32700() {
        let response = handle_mcp_request("not valid json");
        assert!(response.contains("-32700"));
    }

    // ----- time helpers -----

    #[test]
    fn rfc3339_formats_epoch_zero_correctly() {
        assert_eq!(rfc3339_from_epoch_seconds(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_formats_known_timestamp() {
        // 2000-01-01T00:00:00Z == 946684800
        assert_eq!(
            rfc3339_from_epoch_seconds(946684800),
            "2000-01-01T00:00:00Z"
        );
    }

    #[test]
    fn rfc3339_handles_day_boundary() {
        // 1970-01-02T00:00:00Z == 86400
        assert_eq!(rfc3339_from_epoch_seconds(86400), "1970-01-02T00:00:00Z");
    }

    #[test]
    fn classification_labels_match_serde_snake_case() {
        assert_eq!(classification_label(MutationClassification::Read), "read");
        assert_eq!(
            classification_label(MutationClassification::LowRisk),
            "low_risk"
        );
        assert_eq!(
            classification_label(MutationClassification::DataMoving),
            "data_moving"
        );
    }
}
