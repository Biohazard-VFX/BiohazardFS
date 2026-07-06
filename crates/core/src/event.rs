//! Audit events (METADATA_SCHEMA.md "Audit events"). Indexed envelope columns
//! plus a schema-versioned typed JSON payload. Audit events must never contain
//! secrets; the daemon buffers them locally while offline and retries on
//! reconnect. The wire `EventEnvelope` for the live stream lives in
//! `biohazardfs-api-types`; this is the durable audit record.

use crate::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditEventResult {
    Success,
    Failure,
    Partial,
    Queued,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub org_id: String,
    pub audit_event_id: String,
    pub event_type: String,
    pub schema_version: String,
    pub actor_user_id: Option<String>,
    pub impersonated_user_id: Option<String>,
    pub device_id: Option<String>,
    pub source: Source,
    pub request_id: Option<String>,
    pub operation_id: Option<String>,
    pub project_id: Option<String>,
    pub workset_id: Option<String>,
    pub node_id: Option<String>,
    pub version_id: Option<String>,
    pub path_snapshot: Option<String>,
    pub result: AuditEventResult,
    pub created_at: String,
    /// Schema-versioned JSON payload. Must not contain secrets.
    pub payload_json: Option<String>,
}

pub const AUDIT_PAYLOAD_SCHEMA_VERSION: &str = "2026-07-audit-v1";

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::Source;

    #[test]
    fn audit_event_round_trips() {
        let event = AuditEvent {
            org_id: "org_a".into(),
            audit_event_id: "aud_1".into(),
            event_type: "file.write".into(),
            schema_version: AUDIT_PAYLOAD_SCHEMA_VERSION.into(),
            actor_user_id: Some("usr_a".into()),
            impersonated_user_id: None,
            device_id: Some("dev_a".into()),
            source: Source::Cli,
            request_id: Some("req_1".into()),
            operation_id: Some("op_1".into()),
            project_id: None,
            workset_id: None,
            node_id: Some("node_f".into()),
            version_id: Some("ver_2".into()),
            path_snapshot: Some("/Project/Shot.exr".into()),
            result: AuditEventResult::Success,
            created_at: "2026-07-05T00:00:00Z".into(),
            payload_json: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
        assert!(json.contains("\"result\":\"success\""));
    }
}
