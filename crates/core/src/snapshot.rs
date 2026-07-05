//! Snapshots (METADATA_SCHEMA.md "Snapshots"). Point-in-time captures scoped to
//! org, project, workset, or subtree. Read-only; restore is data-moving and
//! audited and copies/promotes data without destroying current data by default.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotScopeKind {
    Org,
    Project,
    Workset,
    Subtree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotStatus {
    Creating,
    Ready,
    Failed,
    Expired,
    Purged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotSource {
    Manual,
    Schedule,
    Preflight,
    Agent,
    Server,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub org_id: String,
    pub snapshot_id: String,
    pub scope_kind: SnapshotScopeKind,
    pub scope_id: Option<String>,
    pub root_node_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub source: SnapshotSource,
    pub retention_policy_id: Option<String>,
    /// Materialized tree reference, version map, or storage snapshot reference.
    pub state_ref: String,
    pub status: SnapshotStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_status_and_scope_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(SnapshotStatus::Creating).unwrap(),
            serde_json::Value::String("creating".into())
        );
        assert_eq!(
            serde_json::to_value(SnapshotScopeKind::Workset).unwrap(),
            serde_json::Value::String("workset".into())
        );
    }

    #[test]
    fn snapshot_round_trips() {
        let snapshot = Snapshot {
            org_id: "org_a".into(),
            snapshot_id: "snap_1".into(),
            scope_kind: SnapshotScopeKind::Project,
            scope_id: Some("proj_a".into()),
            root_node_id: Some("node_root".into()),
            name: "v001".into(),
            description: None,
            created_at: "2026-07-05T00:00:00Z".into(),
            created_by: Some("usr_a".into()),
            source: SnapshotSource::Manual,
            retention_policy_id: None,
            state_ref: "obj_snap_1".into(),
            status: SnapshotStatus::Ready,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
    }
}
