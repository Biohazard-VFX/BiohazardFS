//! Single source of truth for daemon method names, server operation names, and
//! CLI command names, plus mutation classification.
//!
//! This registry exists to kill cross-crate name drift: the daemon's
//! `daemon.methods` response, the server's methods surface, and the CLI
//! `schema list` / `commands` output all read from these slices instead of
//! keeping three hand-edited lists. Today the repo already drifts (daemon
//! advertises `workspace.status`, CLI advertises `daemon.workspace.status`);
//! consumers should adopt the canonical names here.
//!
//! See `docs/architecture/DAEMON_API.md` (method groups),
//! `docs/architecture/SERVER_API.md`, and `docs/reference/COMMANDS.md` for the
//! contract these names are derived from.

use crate::MutationClassification;

/// Which surface a name belongs to. A logical operation may appear on more than
/// one surface (e.g. `cache.pin` is both a daemon method and a CLI command).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Daemon,
    Server,
    Cli,
}

/// Static descriptor for one operation on one surface.
#[derive(Debug, Clone, Copy)]
pub struct MethodDescriptor {
    pub name: &'static str,
    pub group: &'static str,
    pub surface: Surface,
    pub classification: MutationClassification,
    pub summary: &'static str,
}

impl MethodDescriptor {
    const fn daemon(
        name: &'static str,
        group: &'static str,
        classification: MutationClassification,
        summary: &'static str,
    ) -> Self {
        Self {
            name,
            group,
            surface: Surface::Daemon,
            classification,
            summary,
        }
    }

    const fn server(
        name: &'static str,
        group: &'static str,
        classification: MutationClassification,
        summary: &'static str,
    ) -> Self {
        Self {
            name,
            group,
            surface: Surface::Server,
            classification,
            summary,
        }
    }

    const fn cli(
        name: &'static str,
        group: &'static str,
        classification: MutationClassification,
        summary: &'static str,
    ) -> Self {
        Self {
            name,
            group,
            surface: Surface::Cli,
            classification,
            summary,
        }
    }
}

use MutationClassification as M;

/// Every daemon RPC method (DAEMON_API.md method groups). The daemon's
/// `daemon.methods` response and dispatch table must stay within this set.
pub const DAEMON_METHODS: &[MethodDescriptor] = &[
    // daemon/runtime
    MethodDescriptor::daemon("daemon.status", "daemon", M::Read, "daemon runtime status"),
    MethodDescriptor::daemon("daemon.health", "daemon", M::Read, "daemon health checks"),
    MethodDescriptor::daemon("daemon.version", "daemon", M::Read, "daemon version"),
    MethodDescriptor::daemon("daemon.shutdown", "daemon", M::Admin, "stop the daemon"),
    MethodDescriptor::daemon("daemon.restart", "daemon", M::Admin, "restart the daemon"),
    MethodDescriptor::daemon("daemon.logs", "daemon", M::Read, "recent daemon logs"),
    MethodDescriptor::daemon(
        "daemon.events.subscribe",
        "daemon",
        M::Read,
        "subscribe to event stream",
    ),
    MethodDescriptor::daemon(
        "daemon.methods",
        "daemon",
        M::Read,
        "list implemented methods",
    ),
    // workspace runtime (existing scaffold)
    MethodDescriptor::daemon(
        "workspace.status",
        "workspace",
        M::Read,
        "workspace root status",
    ),
    MethodDescriptor::daemon(
        "workspace.list",
        "workspace",
        M::Read,
        "list workspace entries",
    ),
    // auth/session
    MethodDescriptor::daemon("auth.status", "auth", M::Read, "local auth/session state"),
    MethodDescriptor::daemon("auth.enroll", "auth", M::LowRisk, "enroll device"),
    MethodDescriptor::daemon(
        "auth.login_token",
        "auth",
        M::LowRisk,
        "store a login token",
    ),
    MethodDescriptor::daemon("auth.logout", "auth", M::Admin, "clear local session"),
    MethodDescriptor::daemon("auth.whoami", "auth", M::Read, "current actor"),
    MethodDescriptor::daemon(
        "auth.credentials_path",
        "auth",
        M::Read,
        "credentials file path",
    ),
    MethodDescriptor::daemon(
        "auth.rotate_credentials",
        "auth",
        M::LowRisk,
        "rotate local credentials",
    ),
    // config
    MethodDescriptor::daemon("config.path", "config", M::Read, "config file path"),
    MethodDescriptor::daemon("config.show", "config", M::Read, "redacted config"),
    MethodDescriptor::daemon("config.get", "config", M::Read, "read a config value"),
    MethodDescriptor::daemon("config.set", "config", M::LowRisk, "write a config value"),
    MethodDescriptor::daemon("config.validate", "config", M::Read, "validate config"),
    MethodDescriptor::daemon(
        "config.migrate",
        "config",
        M::LowRisk,
        "migrate config schema",
    ),
    // mount
    MethodDescriptor::daemon("mount.status", "mount", M::Read, "mount state"),
    MethodDescriptor::daemon("mount.attach", "mount", M::LowRisk, "attach a mount"),
    MethodDescriptor::daemon("mount.detach", "mount", M::Destructive, "detach a mount"),
    MethodDescriptor::daemon("mount.list", "mount", M::Read, "list mounts"),
    MethodDescriptor::daemon(
        "mount.repair",
        "mount",
        M::DataMoving,
        "repair a broken mount",
    ),
    // file
    MethodDescriptor::daemon("file.stat", "file", M::Read, "file metadata"),
    MethodDescriptor::daemon("file.list", "file", M::Read, "list directory children"),
    MethodDescriptor::daemon("file.history", "file", M::Read, "file audit history"),
    MethodDescriptor::daemon("file.versions", "file", M::Read, "file versions"),
    MethodDescriptor::daemon(
        "file.restore",
        "file",
        M::DataMoving,
        "restore a file version",
    ),
    MethodDescriptor::daemon("file.delete", "file", M::Destructive, "delete to trash"),
    MethodDescriptor::daemon("file.move", "file", M::DataMoving, "rename or move a file"),
    MethodDescriptor::daemon("file.rename", "file", M::LowRisk, "rename a mounted node"),
    MethodDescriptor::daemon(
        "file.mkdir",
        "file",
        M::LowRisk,
        "create a mounted directory",
    ),
    MethodDescriptor::daemon("file.copy", "file", M::DataMoving, "copy a file"),
    MethodDescriptor::daemon("file.checksum", "file", M::Read, "compute file checksum"),
    MethodDescriptor::daemon("file.write", "file", M::LowRisk, "commit a file write"),
    MethodDescriptor::daemon("file.read", "file", M::Read, "read file bytes"),
    // cache
    MethodDescriptor::daemon("cache.status", "cache", M::Read, "cache usage and state"),
    MethodDescriptor::daemon("cache.list", "cache", M::Read, "list cache entries"),
    MethodDescriptor::daemon("cache.pin", "cache", M::LowRisk, "pin a path"),
    MethodDescriptor::daemon("cache.unpin", "cache", M::LowRisk, "unpin a path"),
    MethodDescriptor::daemon("cache.hydrate", "cache", M::LowRisk, "hydrate a file"),
    MethodDescriptor::daemon("cache.dehydrate", "cache", M::LowRisk, "remove local copy"),
    MethodDescriptor::daemon(
        "cache.evict",
        "cache",
        M::Destructive,
        "evict cache entries",
    ),
    MethodDescriptor::daemon("cache.move", "cache", M::DataMoving, "move cache location"),
    MethodDescriptor::daemon("cache.verify", "cache", M::Read, "verify cache integrity"),
    MethodDescriptor::daemon("cache.repair", "cache", M::DataMoving, "repair cache state"),
    // sync
    MethodDescriptor::daemon(
        "sync.status",
        "sync",
        M::Read,
        "server sync configuration status",
    ),
    MethodDescriptor::daemon(
        "sync.push",
        "sync",
        M::DataMoving,
        "push local namespace/content to server",
    ),
    MethodDescriptor::daemon(
        "sync.pull",
        "sync",
        M::DataMoving,
        "pull server namespace/content into local cache",
    ),
    // transfer
    MethodDescriptor::daemon("transfer.list", "transfer", M::Read, "list transfers"),
    MethodDescriptor::daemon("transfer.status", "transfer", M::Read, "transfer status"),
    MethodDescriptor::daemon("transfer.pause", "transfer", M::LowRisk, "pause a transfer"),
    MethodDescriptor::daemon(
        "transfer.resume",
        "transfer",
        M::LowRisk,
        "resume a transfer",
    ),
    MethodDescriptor::daemon(
        "transfer.cancel",
        "transfer",
        M::Destructive,
        "cancel a transfer",
    ),
    MethodDescriptor::daemon("transfer.retry", "transfer", M::LowRisk, "retry a transfer"),
    // snapshot
    MethodDescriptor::daemon("snapshot.list", "snapshot", M::Read, "list snapshots"),
    MethodDescriptor::daemon(
        "snapshot.create",
        "snapshot",
        M::DataMoving,
        "create a snapshot",
    ),
    MethodDescriptor::daemon(
        "snapshot.mount",
        "snapshot",
        M::DataMoving,
        "mount a snapshot read-only",
    ),
    MethodDescriptor::daemon(
        "snapshot.unmount",
        "snapshot",
        M::LowRisk,
        "unmount a snapshot",
    ),
    MethodDescriptor::daemon(
        "snapshot.diff",
        "snapshot",
        M::Read,
        "diff against a snapshot",
    ),
    MethodDescriptor::daemon(
        "snapshot.restore",
        "snapshot",
        M::DataMoving,
        "restore from a snapshot",
    ),
    // lock
    MethodDescriptor::daemon("lock.list", "lock", M::Read, "list locks"),
    MethodDescriptor::daemon("lock.acquire", "lock", M::LowRisk, "acquire a lock"),
    MethodDescriptor::daemon("lock.release", "lock", M::LowRisk, "release a lock"),
    MethodDescriptor::daemon("lock.status", "lock", M::Read, "lock status"),
    MethodDescriptor::daemon("lock.extend", "lock", M::LowRisk, "extend a lock"),
    MethodDescriptor::daemon("lock.break", "lock", M::Admin, "break a lock"),
    // conflict
    MethodDescriptor::daemon("conflict.list", "conflict", M::Read, "list conflicts"),
    MethodDescriptor::daemon("conflict.show", "conflict", M::Read, "show a conflict"),
    MethodDescriptor::daemon(
        "conflict.resolve",
        "conflict",
        M::DataMoving,
        "resolve a conflict",
    ),
    MethodDescriptor::daemon(
        "conflict.preserve_all",
        "conflict",
        M::DataMoving,
        "preserve all sides",
    ),
    // workset
    MethodDescriptor::daemon("workset.list", "workset", M::Read, "list worksets"),
    MethodDescriptor::daemon("workset.show", "workset", M::Read, "show a workset"),
    MethodDescriptor::daemon(
        "workset.activate",
        "workset",
        M::LowRisk,
        "activate a workset",
    ),
    MethodDescriptor::daemon(
        "workset.deactivate",
        "workset",
        M::LowRisk,
        "deactivate a workset",
    ),
    MethodDescriptor::daemon("workset.sync", "workset", M::DataMoving, "sync a workset"),
    MethodDescriptor::daemon("workset.create", "workset", M::LowRisk, "create a workset"),
    MethodDescriptor::daemon("workset.update", "workset", M::LowRisk, "update a workset"),
    // collaboration/share
    MethodDescriptor::daemon("invite.create", "invite", M::LowRisk, "create an invite"),
    MethodDescriptor::daemon("invite.list", "invite", M::Read, "list invites"),
    MethodDescriptor::daemon(
        "invite.revoke",
        "invite",
        M::Destructive,
        "revoke an invite",
    ),
    MethodDescriptor::daemon("share.create", "share", M::LowRisk, "create a share link"),
    MethodDescriptor::daemon("share.list", "share", M::Read, "list shares"),
    MethodDescriptor::daemon("share.revoke", "share", M::Destructive, "revoke a share"),
    MethodDescriptor::daemon("grant.list", "grant", M::Read, "list grants"),
    MethodDescriptor::daemon("grant.set", "grant", M::Admin, "set a grant"),
    MethodDescriptor::daemon("grant.revoke", "grant", M::Admin, "revoke a grant"),
    MethodDescriptor::daemon("publish.create", "publish", M::LowRisk, "publish a version"),
    MethodDescriptor::daemon("publish.list", "publish", M::Read, "list publishes"),
    MethodDescriptor::daemon(
        "publish.revoke",
        "publish",
        M::Destructive,
        "revoke a publish",
    ),
    // audit
    MethodDescriptor::daemon("audit.events", "audit", M::Read, "query audit events"),
    MethodDescriptor::daemon("audit.event", "audit", M::Read, "show one audit event"),
    MethodDescriptor::daemon("audit.actor", "audit", M::Read, "audit by actor"),
    MethodDescriptor::daemon("audit.export", "audit", M::DataMoving, "export audit log"),
    // admin
    MethodDescriptor::daemon("admin.user.list", "admin", M::Admin, "list users"),
    MethodDescriptor::daemon("admin.user.show", "admin", M::Admin, "show a user"),
    MethodDescriptor::daemon("admin.device.list", "admin", M::Admin, "list devices"),
    MethodDescriptor::daemon("admin.device.revoke", "admin", M::Admin, "revoke a device"),
    MethodDescriptor::daemon("admin.token.revoke", "admin", M::Admin, "revoke a token"),
    MethodDescriptor::daemon(
        "admin.retention.show",
        "admin",
        M::Admin,
        "show retention policy",
    ),
    MethodDescriptor::daemon(
        "admin.retention.set",
        "admin",
        M::Admin,
        "set retention policy",
    ),
    MethodDescriptor::daemon(
        "admin.support_bundle.create",
        "admin",
        M::DataMoving,
        "create a support bundle",
    ),
    // schema introspection
    MethodDescriptor::daemon("schema.list", "schema", M::Read, "list method schemas"),
    MethodDescriptor::daemon("schema.method", "schema", M::Read, "describe a method"),
    MethodDescriptor::daemon("schema.event", "schema", M::Read, "describe an event"),
    MethodDescriptor::daemon("schema.error", "schema", M::Read, "describe an error code"),
    MethodDescriptor::daemon("schema.config", "schema", M::Read, "describe config schema"),
    MethodDescriptor::daemon("schema.all", "schema", M::Read, "dump all schemas"),
];

/// Every server/control-plane operation (SERVER_API.md + the route map). Server
/// handlers must stay within this set; the server methods route reads from here.
pub const SERVER_OPERATIONS: &[MethodDescriptor] = &[
    MethodDescriptor::server("server.health", "server", M::Read, "health check"),
    MethodDescriptor::server("server.ready", "server", M::Read, "readiness check"),
    MethodDescriptor::server("server.version", "server", M::Read, "server version"),
    MethodDescriptor::server("server.status", "server", M::Read, "server status"),
    MethodDescriptor::server(
        "server.namespace.children",
        "namespace",
        M::Read,
        "list node children",
    ),
    MethodDescriptor::server(
        "server.objects.content.put",
        "objects",
        M::LowRisk,
        "upload content object",
    ),
    MethodDescriptor::server(
        "server.objects.content.get",
        "objects",
        M::Read,
        "fetch content object",
    ),
    MethodDescriptor::server(
        "server.files.content.put",
        "files",
        M::LowRisk,
        "upload file content",
    ),
    MethodDescriptor::server(
        "server.files.content.get",
        "files",
        M::Read,
        "fetch file content",
    ),
    MethodDescriptor::server("server.nodes.stat", "nodes", M::Read, "node metadata"),
    MethodDescriptor::server(
        "server.nodes.mkdir",
        "nodes",
        M::LowRisk,
        "create a directory",
    ),
    MethodDescriptor::server(
        "server.nodes.symlink",
        "nodes",
        M::LowRisk,
        "create a symlink",
    ),
    MethodDescriptor::server(
        "server.nodes.delete",
        "nodes",
        M::Destructive,
        "delete to trash",
    ),
    MethodDescriptor::server(
        "server.nodes.move",
        "nodes",
        M::DataMoving,
        "rename or move a node",
    ),
    MethodDescriptor::server("server.nodes.copy", "nodes", M::DataMoving, "copy a node"),
    MethodDescriptor::server(
        "server.auth.device.enroll",
        "auth",
        M::LowRisk,
        "enroll a device",
    ),
    MethodDescriptor::server(
        "server.auth.login_token",
        "auth",
        M::LowRisk,
        "issue a login token",
    ),
    MethodDescriptor::server(
        "server.transfers.create",
        "transfers",
        M::LowRisk,
        "issue a transfer token",
    ),
    MethodDescriptor::server(
        "server.transfers.commit",
        "transfers",
        M::LowRisk,
        "commit an upload session",
    ),
    MethodDescriptor::server(
        "server.operations.submit",
        "operations",
        M::LowRisk,
        "submit an offline operation",
    ),
    MethodDescriptor::server(
        "server.operations.replay",
        "operations",
        M::DataMoving,
        "replay operations",
    ),
    MethodDescriptor::server(
        "server.locks.acquire",
        "locks",
        M::LowRisk,
        "acquire a lock",
    ),
    MethodDescriptor::server(
        "server.locks.release",
        "locks",
        M::LowRisk,
        "release a lock",
    ),
    MethodDescriptor::server("server.locks.break", "locks", M::Admin, "break a lock"),
    MethodDescriptor::server("server.locks.list", "locks", M::Read, "list locks"),
    MethodDescriptor::server("server.locks.status", "locks", M::Read, "lock status"),
    MethodDescriptor::server(
        "server.conflicts.list",
        "conflicts",
        M::Read,
        "list conflicts",
    ),
    MethodDescriptor::server(
        "server.conflicts.show",
        "conflicts",
        M::Read,
        "show a conflict",
    ),
    MethodDescriptor::server(
        "server.conflicts.resolve",
        "conflicts",
        M::DataMoving,
        "resolve a conflict",
    ),
    MethodDescriptor::server(
        "server.snapshots.list",
        "snapshots",
        M::Read,
        "list snapshots",
    ),
    MethodDescriptor::server(
        "server.snapshots.create",
        "snapshots",
        M::DataMoving,
        "create a snapshot",
    ),
    MethodDescriptor::server(
        "server.snapshots.mount",
        "snapshots",
        M::DataMoving,
        "mount a snapshot",
    ),
    MethodDescriptor::server(
        "server.snapshots.unmount",
        "snapshots",
        M::LowRisk,
        "unmount a snapshot",
    ),
    MethodDescriptor::server(
        "server.snapshots.diff",
        "snapshots",
        M::Read,
        "diff a snapshot",
    ),
    MethodDescriptor::server(
        "server.snapshots.restore",
        "snapshots",
        M::DataMoving,
        "restore from snapshot",
    ),
    MethodDescriptor::server("server.grants.list", "grants", M::Read, "list grants"),
    MethodDescriptor::server("server.grants.set", "grants", M::Admin, "set a grant"),
    MethodDescriptor::server("server.grants.revoke", "grants", M::Admin, "revoke a grant"),
    MethodDescriptor::server(
        "server.shares.create",
        "shares",
        M::LowRisk,
        "create a share",
    ),
    MethodDescriptor::server("server.shares.list", "shares", M::Read, "list shares"),
    MethodDescriptor::server(
        "server.shares.revoke",
        "shares",
        M::Destructive,
        "revoke a share",
    ),
    MethodDescriptor::server(
        "server.publishes.create",
        "publishes",
        M::LowRisk,
        "publish a version",
    ),
    MethodDescriptor::server(
        "server.publishes.list",
        "publishes",
        M::Read,
        "list publishes",
    ),
    MethodDescriptor::server(
        "server.publishes.revoke",
        "publishes",
        M::Destructive,
        "revoke a publish",
    ),
    MethodDescriptor::server(
        "server.invites.create",
        "invites",
        M::LowRisk,
        "create an invite",
    ),
    MethodDescriptor::server("server.invites.list", "invites", M::Read, "list invites"),
    MethodDescriptor::server(
        "server.invites.revoke",
        "invites",
        M::Destructive,
        "revoke an invite",
    ),
    MethodDescriptor::server("server.devices.list", "devices", M::Read, "list devices"),
    MethodDescriptor::server(
        "server.devices.revoke",
        "devices",
        M::Admin,
        "revoke a device",
    ),
    MethodDescriptor::server("server.trash.list", "trash", M::Read, "list trash"),
    MethodDescriptor::server(
        "server.trash.restore",
        "trash",
        M::DataMoving,
        "restore from trash",
    ),
    MethodDescriptor::server("server.trash.purge", "trash", M::Admin, "purge trash"),
    MethodDescriptor::server(
        "server.audit.events",
        "audit",
        M::Read,
        "query audit events",
    ),
    MethodDescriptor::server(
        "server.audit.event",
        "audit",
        M::Read,
        "show one audit event",
    ),
    MethodDescriptor::server("server.audit.actor", "audit", M::Read, "audit by actor"),
    MethodDescriptor::server(
        "server.audit.export",
        "audit",
        M::DataMoving,
        "export audit log",
    ),
    MethodDescriptor::server("server.projects.list", "projects", M::Read, "list projects"),
    MethodDescriptor::server(
        "server.projects.create",
        "projects",
        M::LowRisk,
        "create a project",
    ),
    MethodDescriptor::server("server.worksets.list", "worksets", M::Read, "list worksets"),
    MethodDescriptor::server(
        "server.worksets.create",
        "worksets",
        M::LowRisk,
        "create a workset",
    ),
];

/// CLI-only command names that do not mirror a daemon method or server
/// operation. Most CLI commands are derived from `DAEMON_METHODS` (same name);
/// this list adds the CLI-specific surfaces.
pub const CLI_ONLY_COMMANDS: &[MethodDescriptor] = &[
    MethodDescriptor::cli(
        "client.status",
        "client",
        M::Read,
        "client + daemon reachability",
    ),
    MethodDescriptor::cli("client.version", "client", M::Read, "client version"),
    MethodDescriptor::cli("version", "version", M::Read, "product version"),
    MethodDescriptor::cli("doctor", "doctor", M::Read, "diagnose install"),
    MethodDescriptor::cli("smoke.run", "smoke", M::Read, "run validation smoke"),
    MethodDescriptor::cli("mcp.serve", "mcp", M::Read, "stdio MCP server"),
    MethodDescriptor::cli("schema.list", "schema", M::Read, "list command schemas"),
    MethodDescriptor::cli("schema.command", "schema", M::Read, "describe a command"),
    MethodDescriptor::cli("schema.event", "schema", M::Read, "describe an event"),
    MethodDescriptor::cli("schema.error", "schema", M::Read, "describe an error code"),
    MethodDescriptor::cli("schema.config", "schema", M::Read, "describe config schema"),
    MethodDescriptor::cli("schema.all", "schema", M::Read, "dump all schemas"),
];

/// Find a descriptor by surface + canonical name.
pub fn find(surface: Surface, name: &str) -> Option<MethodDescriptor> {
    match surface {
        Surface::Daemon => DAEMON_METHODS.iter().copied().find(|d| d.name == name),
        Surface::Server => SERVER_OPERATIONS.iter().copied().find(|d| d.name == name),
        Surface::Cli => cli_commands().iter().copied().find(|d| d.name == name),
    }
}

/// Classify a method by surface + name. Unknown names default to `Read` for
/// safety of read paths but callers should treat unknown mutating names as
/// requiring a token (see COMMANDS.md mutation policy).
pub fn classify(surface: Surface, name: &str) -> MutationClassification {
    find(surface, name)
        .map(|d| d.classification)
        .unwrap_or(MutationClassification::Read)
}

/// Daemon method names, sorted, deduped. Returned as owned `String`s because
/// the daemon `daemon.methods` envelope serializes them.
pub fn daemon_method_names() -> Vec<String> {
    let mut names: Vec<String> = DAEMON_METHODS.iter().map(|d| d.name.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

/// Server operation names, sorted, deduped.
pub fn server_operation_names() -> Vec<String> {
    let mut names: Vec<String> = SERVER_OPERATIONS
        .iter()
        .map(|d| d.name.to_string())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// All CLI command names: daemon methods (most are CLI-exposable) plus the
/// CLI-only surfaces. Sorted, deduped.
pub fn cli_command_names() -> Vec<String> {
    let mut names: Vec<String> = cli_commands().iter().map(|d| d.name.to_string()).collect();
    names.sort();
    names.dedup();
    names
}

/// CLI commands are the union of daemon methods (most mirror a CLI subcommand)
/// and the CLI-only surfaces. Computed rather than re-listed to avoid drift.
pub fn cli_commands() -> &'static [MethodDescriptor] {
    use std::sync::OnceLock;
    static CLI: OnceLock<Vec<MethodDescriptor>> = OnceLock::new();
    CLI.get_or_init(|| {
        let mut all: Vec<MethodDescriptor> = DAEMON_METHODS
            .iter()
            .copied()
            .map(|d| MethodDescriptor {
                surface: Surface::Cli,
                ..d
            })
            .chain(CLI_ONLY_COMMANDS.iter().copied())
            .collect();
        all.sort_by_key(|d| d.name);
        all.dedup_by_key(|d| d.name);
        all
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_method_names_include_workspace_canonical_form() {
        let names = daemon_method_names();
        assert!(names.contains(&"workspace.status".to_string()), "{names:?}");
        assert!(names.contains(&"workspace.list".to_string()), "{names:?}");
    }

    #[test]
    fn classify_knows_data_moving_methods() {
        assert_eq!(classify(Surface::Daemon, "file.restore"), M::DataMoving);
        assert_eq!(classify(Surface::Daemon, "lock.break"), M::Admin);
        assert_eq!(classify(Surface::Daemon, "file.delete"), M::Destructive);
        assert_eq!(classify(Surface::Daemon, "cache.status"), M::Read);
        assert_eq!(classify(Surface::Daemon, "cache.pin"), M::LowRisk);
    }

    #[test]
    fn unknown_methods_default_to_read() {
        assert_eq!(classify(Surface::Daemon, "nope.does_not_exist"), M::Read);
    }

    #[test]
    fn cli_commands_include_cli_only_and_daemon_mirrors() {
        let names = cli_command_names();
        assert!(names.contains(&"client.status".to_string()));
        assert!(names.contains(&"doctor".to_string()));
        assert!(names.contains(&"cache.pin".to_string()));
        assert!(names.contains(&"mcp.serve".to_string()));
    }

    #[test]
    fn no_duplicate_names_within_a_surface_slice() {
        let check = |slice: &[MethodDescriptor]| {
            let mut names: Vec<&str> = slice.iter().map(|d| d.name).collect();
            let total = names.len();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), total, "duplicate method names in slice");
        };
        check(DAEMON_METHODS);
        check(SERVER_OPERATIONS);
        check(CLI_ONLY_COMMANDS);
    }
}
