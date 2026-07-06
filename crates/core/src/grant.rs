//! Grants, shares, publishes (METADATA_SCHEMA.md "Grants and permissions" +
//! "Shares and publishes"). Most users get access via project/workset grants;
//! node grants are targeted overrides; share grants model external access.

use crate::Source;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    Hidden,
    Read,
    Write,
    Admin,
    Share,
    Publish,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubjectKind {
    User,
    Group,
    Device,
    Token,
    Invite,
    Share,
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Project,
    Workset,
    Node,
    Share,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub org_id: String,
    pub grant_id: String,
    pub subject_kind: SubjectKind,
    pub subject_id: String,
    pub resource_kind: ResourceKind,
    pub resource_id: String,
    pub permissions: Vec<Permission>,
    pub expires_at: Option<String>,
    pub constraints_json: Option<String>,
    pub created_at: String,
    pub created_by: Option<String>,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareAccessMode {
    Read,
    Write,
    Review,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub org_id: String,
    pub share_id: String,
    pub created_by: Option<String>,
    pub resource_kind: ResourceKind,
    pub resource_id: String,
    pub access_mode: ShareAccessMode,
    pub expires_at: Option<String>,
    pub constraints_json: Option<String>,
    pub status: ShareStatus,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PublishStatus {
    Active,
    Superseded,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Publish {
    pub org_id: String,
    pub publish_id: String,
    pub project_id: Option<String>,
    pub node_id: String,
    pub version_id: String,
    pub label: String,
    pub comment: Option<String>,
    pub created_by: Option<String>,
    pub created_device_id: Option<String>,
    pub source: Source,
    pub status: PublishStatus,
    pub created_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::Source;

    #[test]
    fn permission_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(Permission::Hidden).unwrap(),
            serde_json::Value::String("hidden".into())
        );
    }

    #[test]
    fn grant_round_trips() {
        let grant = Grant {
            org_id: "org_a".into(),
            grant_id: "grant_1".into(),
            subject_kind: SubjectKind::User,
            subject_id: "usr_a".into(),
            resource_kind: ResourceKind::Project,
            resource_id: "proj_a".into(),
            permissions: vec![Permission::Read, Permission::Write],
            expires_at: None,
            constraints_json: None,
            created_at: "2026-07-05T00:00:00Z".into(),
            created_by: Some("usr_admin".into()),
            revoked_at: None,
            revoked_by: None,
        };
        let json = serde_json::to_string(&grant).unwrap();
        let back: Grant = serde_json::from_str(&json).unwrap();
        assert_eq!(back, grant);
        assert!(json.contains("\"permissions\":[\"read\",\"write\"]"));
    }

    #[test]
    fn publish_round_trips_with_source() {
        let publish = Publish {
            org_id: "org_a".into(),
            publish_id: "pub_1".into(),
            project_id: Some("proj_a".into()),
            node_id: "node_f".into(),
            version_id: "ver_2".into(),
            label: "v002".into(),
            comment: None,
            created_by: Some("usr_a".into()),
            created_device_id: Some("dev_a".into()),
            source: Source::Ui,
            status: PublishStatus::Active,
            created_at: "2026-07-05T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&publish).unwrap();
        let back: Publish = serde_json::from_str(&json).unwrap();
        assert_eq!(back, publish);
        assert!(json.contains("\"source\":\"ui\""));
    }
}
