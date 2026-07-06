//! File locks (METADATA_SCHEMA.md "Locks"). Existing files lock by `node_id`;
//! offline-created files use `provisional_local_id` until a server node ID is
//! assigned. `path_snapshot` is display/audit only and never defines identity
//! when a node_id is present.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockKind {
    Edit,
    Admin,
    Publish,
    Restore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockStatus {
    Active,
    Released,
    Expired,
    Broken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileLock {
    pub org_id: String,
    pub lock_id: String,
    pub node_id: Option<String>,
    pub provisional_local_id: Option<String>,
    pub path_snapshot: String,
    pub owner_user_id: Option<String>,
    pub owner_device_id: Option<String>,
    pub kind: LockKind,
    pub status: LockStatus,
    pub acquired_at: String,
    /// Lazy expiry: a lock past `expires_at` is treated as Expired on next
    /// access (FILESYSTEM_SEMANTICS.md). No background sweep in v1.
    pub expires_at: Option<String>,
    pub released_at: Option<String>,
    pub broken_at: Option<String>,
    pub broken_by: Option<String>,
    pub operation_id: Option<String>,
}

impl FileLock {
    /// True when the lock is Active and not past its expiry. Expiry is lazy;
    /// callers must treat `false` as "do not honor" without mutating state.
    pub fn is_effective_at(&self, now_rfc3339: &str) -> bool {
        if self.status != LockStatus::Active {
            return false;
        }
        match &self.expires_at {
            Some(expires_at) => expires_at.as_str() > now_rfc3339,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_active_lock_is_not_effective() {
        let mut lock = FileLock {
            org_id: "org_a".into(),
            lock_id: "lock_1".into(),
            node_id: Some("node_f".into()),
            provisional_local_id: None,
            path_snapshot: "/Project/Shot.exr".into(),
            owner_user_id: Some("usr_a".into()),
            owner_device_id: Some("dev_a".into()),
            kind: LockKind::Edit,
            status: LockStatus::Active,
            acquired_at: "2026-07-05T00:00:00Z".into(),
            expires_at: Some("2026-07-05T01:00:00Z".into()),
            released_at: None,
            broken_at: None,
            broken_by: None,
            operation_id: None,
        };
        assert!(lock.is_effective_at("2026-07-05T00:30:00Z"));
        assert!(!lock.is_effective_at("2026-07-05T02:00:00Z"));
        lock.status = LockStatus::Released;
        assert!(!lock.is_effective_at("2026-07-05T00:30:00Z"));
    }

    #[test]
    fn lock_kind_serializes_snake_case() {
        let value = serde_json::to_value(LockKind::Publish).unwrap();
        assert_eq!(value, serde_json::Value::String("publish".to_string()));
    }
}
