//! Offline/client operation log (METADATA_SCHEMA.md "Offline/client operation
//! log"). Daemons submit queued offline operations with base IDs/versions and
//! idempotency keys; the server records each before applying or rejecting.
//!
//! Replay of submitted operations is scaffold-depth: the daemon/server issue
//! and record tokens and validate presence on apply, but a full replay engine
//! is deferred and surfaces as a typed `not_implemented_offline_replay` error.

use crate::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Received,
    Accepted,
    Applied,
    Rejected,
    Conflicted,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    pub org_id: String,
    pub operation_id: String,
    pub client_operation_id: String,
    pub device_id: Option<String>,
    pub actor_user_id: Option<String>,
    pub impersonated_user_id: Option<String>,
    pub source: Source,
    /// Daemon method or server operation name this record applies.
    pub method: String,
    /// Opaque JSON of the method params; validated against the method schema.
    pub params_json: String,
    pub base_node_id: Option<String>,
    pub base_version_id: Option<String>,
    pub base_snapshot_id: Option<String>,
    pub idempotency_key: String,
    pub status: OperationStatus,
    pub result_json: Option<String>,
    pub conflict_id: Option<String>,
    pub created_at_client: String,
    pub received_at_server: Option<String>,
    pub applied_at_server: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::Source;

    #[test]
    fn operation_status_serializes_snake_case() {
        let value = serde_json::to_value(OperationStatus::Conflicted).unwrap();
        assert_eq!(value, serde_json::Value::String("conflicted".to_string()));
    }

    #[test]
    fn operation_round_trips() {
        let operation = Operation {
            org_id: "org_a".into(),
            operation_id: "op_1".into(),
            client_operation_id: "cop_local_1".into(),
            device_id: Some("dev_a".into()),
            actor_user_id: Some("usr_a".into()),
            impersonated_user_id: None,
            source: Source::Cli,
            method: "file.write".into(),
            params_json: "{}".into(),
            base_node_id: Some("node_f".into()),
            base_version_id: Some("ver_1".into()),
            base_snapshot_id: None,
            idempotency_key: "idem_1".into(),
            status: OperationStatus::Received,
            result_json: None,
            conflict_id: None,
            created_at_client: "2026-07-05T00:00:00Z".into(),
            received_at_server: None,
            applied_at_server: None,
        };
        let json = serde_json::to_string(&operation).unwrap();
        let back: Operation = serde_json::from_str(&json).unwrap();
        assert_eq!(back, operation);
    }
}
