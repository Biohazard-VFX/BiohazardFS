//! Namespace nodes (METADATA_SCHEMA.md "Namespace nodes").
//!
//! Identity is the stable `node_id`; path is derived from mutable parent +
//! name. Nodes are soft-deleted via `deleted_at` + `trash_id`; the namespace
//! read path filters deleted nodes out unless explicitly querying trash.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeKind {
    File,
    Directory,
    Symlink,
}

/// Logical namespace node. Field names mirror METADATA_SCHEMA.md so the server
/// INSERT/SELECT mapping is mechanical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Node {
    pub org_id: String,
    pub node_id: String,
    pub project_id: Option<String>,
    pub parent_node_id: Option<String>,
    pub name: String,
    pub kind: NodeKind,
    /// Current visible version for files; None for directories/symlinks.
    pub current_version_id: Option<String>,
    /// Symlink target; required for `Symlink`, constrained to authorized roots.
    pub target: Option<String>,
    /// POSIX-style permission bits (e.g. 0o644). Stored as a string so the wire
    /// shape never loses precision to a JSON number.
    pub mode: Option<String>,
    pub owner_user_id: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub updated_at: String,
    pub updated_by: Option<String>,
    pub deleted_at: Option<String>,
    pub deleted_by: Option<String>,
    pub trash_id: Option<String>,
    /// Advisory rebuildable display path; never authoritative for identity.
    pub path_cache: Option<String>,
}

impl Node {
    pub fn is_live(&self) -> bool {
        self.deleted_at.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_kind_serializes_snake_case() {
        let value = serde_json::to_value(NodeKind::Directory).unwrap();
        assert_eq!(value, serde_json::Value::String("directory".to_string()));
    }

    #[test]
    fn node_round_trips() {
        let node = Node {
            org_id: "org_a".into(),
            node_id: "node_root".into(),
            project_id: None,
            parent_node_id: None,
            name: "Project".into(),
            kind: NodeKind::Directory,
            current_version_id: None,
            target: None,
            mode: Some("0o755".into()),
            owner_user_id: Some("usr_a".into()),
            created_at: "2026-07-05T00:00:00Z".into(),
            created_by: Some("usr_a".into()),
            updated_at: "2026-07-05T00:00:00Z".into(),
            updated_by: None,
            deleted_at: None,
            deleted_by: None,
            trash_id: None,
            path_cache: Some("/Project".into()),
        };
        let json = serde_json::to_string(&node).unwrap();
        let back: Node = serde_json::from_str(&json).unwrap();
        assert_eq!(back, node);
        assert!(back.is_live());
    }
}
