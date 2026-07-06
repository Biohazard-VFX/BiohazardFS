//! File versions and content manifests (METADATA_SCHEMA.md "File versions and
//! content manifests"). File versions are immutable; a node's
//! `current_version_id` points at the visible one. Restores create or promote a
//! version through an audited operation and never mutate an old version.

use crate::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentManifestRef {
    /// Logical object/manifest ID (e.g. `obj_...`) resolved through the server.
    pub object_id: String,
    /// Object-store key/path. Org-scoped on the server.
    pub storage_key: String,
    /// Chunking strategy/version, if chunked; None for single-blob v1.
    pub chunking: Option<String>,
}

/// Immutable file version. Once written, fields never change; corrections ship
/// as a new version with `parent_version_id` pointing back here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileVersion {
    pub org_id: String,
    pub version_id: String,
    pub node_id: String,
    pub parent_version_id: Option<String>,
    pub content_manifest_ref: ContentManifestRef,
    pub content_hash: String,
    pub size_bytes: u64,
    /// Logical mtime as seen by the filesystem, RFC3339 UTC.
    pub logical_mtime: String,
    pub created_at: String,
    pub created_by: Option<String>,
    pub created_device_id: Option<String>,
    pub source: Source,
    pub operation_id: Option<String>,
    pub audit_event_id: Option<String>,
    /// Free-form app metadata (DCC tags, etc.); opaque JSON string.
    pub metadata_json: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::Source;

    #[test]
    fn version_round_trips_with_source() {
        let version = FileVersion {
            org_id: "org_a".into(),
            version_id: "ver_1".into(),
            node_id: "node_f".into(),
            parent_version_id: None,
            content_manifest_ref: ContentManifestRef {
                object_id: "obj_1".into(),
                storage_key: "orgs/org_a/content/sha256/abc".into(),
                chunking: None,
            },
            content_hash: "sha256:abc".into(),
            size_bytes: 42,
            logical_mtime: "2026-07-05T00:00:00Z".into(),
            created_at: "2026-07-05T00:00:00Z".into(),
            created_by: Some("usr_a".into()),
            created_device_id: Some("dev_a".into()),
            source: Source::Cli,
            operation_id: None,
            audit_event_id: None,
            metadata_json: None,
        };
        let json = serde_json::to_string(&version).unwrap();
        let back: FileVersion = serde_json::from_str(&json).unwrap();
        assert_eq!(back, version);
        // Source crosses the wire as snake_case.
        assert!(json.contains("\"source\":\"cli\""));
    }
}
