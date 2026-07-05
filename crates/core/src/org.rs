//! Organization, users, devices, tokens, invites, projects, worksets, trash,
//! retention (METADATA_SCHEMA.md). Every primary record is org-scoped.

use crate::Source;
use serde::{Deserialize, Serialize};

// ----- Status enums -----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrgStatus {
    Active,
    Disabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserStatus {
    Active,
    Disabled,
    Invited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Active,
    Revoked,
    Lost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    Device,
    Api,
    Invite,
    Service,
    LocalExchange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenStatus {
    Active,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteStatus {
    Active,
    Revoked,
    Expired,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorksetStatus {
    Active,
    Archived,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorksetSource {
    Manual,
    Integration,
    Invite,
    Share,
    Agent,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorksetRuleKind {
    Node,
    Subtree,
    Pattern,
    Tag,
    IntegrationAssignment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrashRecordStatus {
    Trashed,
    Restored,
    Purged,
}

// ----- Records -----

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Organization {
    pub org_id: String,
    pub slug: String,
    pub display_name: String,
    pub status: OrgStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub org_id: String,
    pub user_id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub role_hint: Option<String>,
    pub status: UserStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub org_id: String,
    pub device_id: String,
    pub user_id: Option<String>,
    pub display_name: String,
    pub platform: String,
    pub hostname: Option<String>,
    pub public_key_ref: Option<String>,
    pub status: DeviceStatus,
    pub enrolled_at: String,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub org_id: String,
    pub token_id: String,
    pub user_id: Option<String>,
    pub device_id: Option<String>,
    pub kind: TokenKind,
    /// Scopes as a JSON array string (e.g. `["file:read","file:write"]`).
    pub scopes: String,
    pub status: TokenStatus,
    pub issued_at: String,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
    pub revoked_by: Option<String>,
    /// `sha256:<hex>` of the raw token secret. Raw secrets are never stored.
    pub secret_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    pub org_id: String,
    pub invite_id: String,
    pub created_by: Option<String>,
    pub intended_email: Option<String>,
    pub default_project_id: Option<String>,
    pub default_workset_id: Option<String>,
    pub scopes_json: Option<String>,
    pub expires_at: Option<String>,
    pub max_uses: Option<u32>,
    pub uses_count: u32,
    pub status: InviteStatus,
    pub created_at: String,
    pub revoked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub org_id: String,
    pub project_id: String,
    pub root_node_id: Option<String>,
    pub name: String,
    pub code: Option<String>,
    pub status: ProjectStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workset {
    pub org_id: String,
    pub workset_id: String,
    pub project_id: Option<String>,
    pub name: String,
    pub description: Option<String>,
    pub status: WorksetStatus,
    pub source: WorksetSource,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorksetRule {
    pub org_id: String,
    pub workset_id: String,
    pub rule_id: String,
    pub kind: WorksetRuleKind,
    pub node_id: Option<String>,
    pub pattern: Option<String>,
    pub permissions_hint: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashRecord {
    pub org_id: String,
    pub trash_id: String,
    pub node_id: String,
    pub original_parent_node_id: Option<String>,
    pub original_name: String,
    pub deleted_version_id: Option<String>,
    pub deleted_at: String,
    pub deleted_by: Option<String>,
    pub operation_id: Option<String>,
    pub purge_after: Option<String>,
    pub purged_at: Option<String>,
    pub purged_by: Option<String>,
    pub status: TrashRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub org_id: String,
    pub retention_policy_id: String,
    pub name: String,
    pub resource_kind: String,
    pub resource_id: Option<String>,
    /// Opaque JSON rules (durations, max-versions, etc.).
    pub rules_json: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Who created a record, for audit. Re-exported convenience alias.
pub type RecordSource = Source;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_kind_and_status_serialize_snake_case() {
        assert_eq!(
            serde_json::to_value(TokenKind::LocalExchange).unwrap(),
            serde_json::Value::String("local_exchange".into())
        );
        assert_eq!(
            serde_json::to_value(TokenStatus::Active).unwrap(),
            serde_json::Value::String("active".into())
        );
    }

    #[test]
    fn device_round_trips() {
        let device = Device {
            org_id: "org_a".into(),
            device_id: "dev_a".into(),
            user_id: Some("usr_a".into()),
            display_name: "workstation".into(),
            platform: "linux".into(),
            hostname: Some("ws".into()),
            public_key_ref: None,
            status: DeviceStatus::Active,
            enrolled_at: "2026-07-05T00:00:00Z".into(),
            last_seen_at: None,
            revoked_at: None,
            revoked_by: None,
        };
        let json = serde_json::to_string(&device).unwrap();
        let back: Device = serde_json::from_str(&json).unwrap();
        assert_eq!(back, device);
    }

    #[test]
    fn workset_rule_kind_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(WorksetRuleKind::IntegrationAssignment).unwrap(),
            serde_json::Value::String("integration_assignment".into())
        );
    }
}
