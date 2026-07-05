//! Daemon transport contract (DAEMON_API.md "Transport").
//!
//! The daemon ships two transports:
//!
//! 1. **Dev/integration:** loopback HTTP JSON-RPC, implemented in `lib.rs`
//!    (`run_dev_loopback_http`). Binds only to 127.0.0.1 / [::1]; requires the
//!    owner-only local session token. This is the only transport wired up in
//!    the scaffold today.
//! 2. **Production (planned):** platform IPC — Unix domain socket on
//!    Linux/macOS, named pipe on Windows. Discovered by clients through an
//!    owner-only runtime descriptor file.
//!
//! This module owns the descriptor shape, the transport kind enum, and the
//! constants that describe the production transport surface. It does not
//! implement the IPC socket itself; that lands in a later hardening pass.
//!
//! The dev-loopback HTTP plumbing (header/body limits, line readers, auth
//! check) intentionally stays in `lib.rs` so the existing CLI/tests that drive
//! the scaffold HTTP transport keep working unchanged in shape.

use serde::{Deserialize, Serialize};

/// Schema version stamped on every daemon endpoint descriptor file.
pub const ENDPOINT_DESCRIPTOR_SCHEMA_VERSION: &str = "2026-07-daemon-endpoint-v1";

/// Default token-file directory inside the owner's runtime state dir. The
/// daemon writes the owner-only local session token here.
pub const DEFAULT_TOKEN_FILE_NAME: &str = "session.token";

/// Default descriptor file name inside the owner's runtime state dir.
pub const DEFAULT_DESCRIPTOR_FILE_NAME: &str = "daemon.json";

/// The transport a daemon endpoint uses. Mirrors the descriptor's `transport`
/// field; the scaffold only advertises the dev-loopback transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportKind {
    /// Linux/macOS Unix domain socket. Reserved; not yet implemented.
    Unix,
    /// Windows named pipe. Reserved; not yet implemented.
    Pipe,
    /// Loopback HTTP JSON-RPC, dev/integration only. Always 127.0.0.1/[::1].
    DevLoopbackHttpJsonRpc,
}

impl TransportKind {
    /// Wire string used in `DaemonStatus.transport` and descriptor files.
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Unix => "unix_domain_socket",
            TransportKind::Pipe => "windows_named_pipe",
            TransportKind::DevLoopbackHttpJsonRpc => "dev_loopback_http_json_rpc",
        }
    }
}

/// Owner-only runtime descriptor clients read to discover the daemon endpoint
/// (DAEMON_API.md "Transport"). Descriptor and token files must be readable
/// only by the owning OS user; that permission boundary is enforced where the
/// file is written, not in this type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportDescriptor {
    pub schema_version: String,
    pub pid: u32,
    pub transport: TransportKind,
    /// Primary IPC endpoint path (Unix socket / named pipe). `None` when only
    /// the dev-loopback HTTP transport is exposed.
    pub endpoint: Option<String>,
    /// Optional dev-loopback HTTP address. `None` unless dev mode is enabled.
    pub http_endpoint: Option<String>,
    /// Path to the owner-only local session token file.
    pub token_file: String,
    /// RFC3339 UTC timestamp the daemon started.
    pub started_at: String,
}

impl TransportDescriptor {
    /// Build a descriptor for the dev-loopback HTTP scaffold transport. The
    /// IPC `endpoint` is `None` because platform IPC is not yet implemented.
    pub fn for_dev_loopback(
        pid: u32,
        http_endpoint: impl Into<String>,
        token_file: impl Into<String>,
        started_at: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: ENDPOINT_DESCRIPTOR_SCHEMA_VERSION.to_string(),
            pid,
            transport: TransportKind::DevLoopbackHttpJsonRpc,
            endpoint: None,
            http_endpoint: Some(http_endpoint.into()),
            token_file: token_file.into(),
            started_at: started_at.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_loopback_descriptor_round_trips() {
        let descriptor = TransportDescriptor::for_dev_loopback(
            42_601,
            "127.0.0.1:47666",
            "/run/user/1000/biohazardfs/session.token",
            "2026-07-05T00:00:00Z",
        );
        let json = serde_json::to_string(&descriptor).expect("descriptor serializes");
        let back: TransportDescriptor =
            serde_json::from_str(&json).expect("descriptor deserializes");
        assert_eq!(back, descriptor);
        assert_eq!(back.transport, TransportKind::DevLoopbackHttpJsonRpc);
        assert_eq!(back.schema_version, ENDPOINT_DESCRIPTOR_SCHEMA_VERSION);
        assert_eq!(back.transport.as_str(), "dev_loopback_http_json_rpc");
        assert!(json.contains("\"transport\":\"dev_loopback_http_json_rpc\""));
        assert!(json.contains("\"endpoint\":null"));
    }

    #[test]
    fn transport_kinds_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(TransportKind::Unix).unwrap(),
            serde_json::Value::String("unix".to_string())
        );
        assert_eq!(
            serde_json::to_value(TransportKind::Pipe).unwrap(),
            serde_json::Value::String("pipe".to_string())
        );
    }
}
