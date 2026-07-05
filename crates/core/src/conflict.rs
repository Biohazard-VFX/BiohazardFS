//! Conflicts (METADATA_SCHEMA.md "Conflicts"). Divergent reconnects always
//! preserve both sides and create conflict records; no silent overwrite.
//! Automatic content merge is out of scope for MVP; resolution is "pick a
//! winner by version" plus optional `preserve_all`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    WriteWrite,
    DeleteWrite,
    RenameRename,
    RenameDelete,
    Permission,
    Lock,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    Open,
    Resolved,
    PreservedAll,
    Dismissed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Conflict {
    pub org_id: String,
    pub conflict_id: String,
    pub node_id: Option<String>,
    pub path_snapshot: String,
    pub kind: ConflictKind,
    pub base_version_id: Option<String>,
    pub local_version_id: Option<String>,
    pub remote_version_id: Option<String>,
    pub local_operation_id: Option<String>,
    pub remote_operation_id: Option<String>,
    pub status: ConflictStatus,
    pub created_at: String,
    pub resolved_at: Option<String>,
    pub resolved_by: Option<String>,
    /// Opaque JSON capturing the chosen resolution for audit.
    pub resolution_json: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_round_trips() {
        let conflict = Conflict {
            org_id: "org_a".into(),
            conflict_id: "conf_1".into(),
            node_id: Some("node_f".into()),
            path_snapshot: "/Project/Shot.exr".into(),
            kind: ConflictKind::WriteWrite,
            base_version_id: Some("ver_1".into()),
            local_version_id: Some("ver_2".into()),
            remote_version_id: Some("ver_3".into()),
            local_operation_id: Some("op_1".into()),
            remote_operation_id: Some("op_2".into()),
            status: ConflictStatus::Open,
            created_at: "2026-07-05T00:00:00Z".into(),
            resolved_at: None,
            resolved_by: None,
            resolution_json: None,
        };
        let json = serde_json::to_string(&conflict).unwrap();
        let back: Conflict = serde_json::from_str(&json).unwrap();
        assert_eq!(back, conflict);
        assert!(json.contains("\"kind\":\"write_write\""));
    }
}
