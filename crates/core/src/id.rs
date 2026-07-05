//! Opaque prefixed IDs (METADATA_SCHEMA.md "ID conventions"). IDs are stored
//! and serialized as plain strings everywhere; these helpers enforce the prefix
//! and character-set invariants at trust boundaries. Typed newtype wrappers are
//! a deliberate later hardening pass, not this layer.

use crate::error::CoreError;

pub const ORG_ID_PREFIX: &str = "org_";
pub const USER_ID_PREFIX: &str = "usr_";
pub const ACCOUNT_ID_PREFIX: &str = "acct_";
pub const DEVICE_ID_PREFIX: &str = "dev_";
pub const TOKEN_ID_PREFIX: &str = "tok_";
pub const INVITE_ID_PREFIX: &str = "inv_";
pub const PROJECT_ID_PREFIX: &str = "proj_";
pub const WORKSET_ID_PREFIX: &str = "wrk_";
pub const NODE_ID_PREFIX: &str = "node_";
pub const VERSION_ID_PREFIX: &str = "ver_";
pub const SNAPSHOT_ID_PREFIX: &str = "snap_";
pub const LOCK_ID_PREFIX: &str = "lock_";
pub const CONFLICT_ID_PREFIX: &str = "conf_";
pub const OPERATION_ID_PREFIX: &str = "op_";
pub const AUDIT_EVENT_ID_PREFIX: &str = "aud_";
pub const SHARE_ID_PREFIX: &str = "share_";
pub const PUBLISH_ID_PREFIX: &str = "pub_";
pub const TRASH_ID_PREFIX: &str = "trash_";
pub const RETENTION_ID_PREFIX: &str = "ret_";
pub const OBJECT_ID_PREFIX: &str = "obj_";
pub const TRANSFER_ID_PREFIX: &str = "xfer_";

/// Maximum body length after the prefix. Long enough for hex timestamps +
/// sequence; short enough to keep indexes and logs bounded.
pub const MAX_ID_BODY_LEN: usize = 64;

/// Valid ID body characters: lowercase ASCII, digits, underscore, hyphen.
/// Uppercase is rejected so IDs collide-free under case-insensitive comparisons.
const VALID_BODY_CHARS: &[char] = &[
    'a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j', 'k', 'l', 'm', 'n', 'o', 'p', 'q', 'r', 's',
    't', 'u', 'v', 'w', 'x', 'y', 'z', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '_', '-',
];

/// Validate that `value` carries `prefix` followed by a non-empty, bounded body
/// of allowed characters. Used at every boundary that accepts an ID from a
/// less-trusted layer (CLI args, daemon params, server query strings).
pub fn validate_id(prefix: &str, value: &str) -> Result<(), CoreError> {
    let body = value.strip_prefix(prefix).ok_or_else(|| {
        CoreError::new(
            "invalid_id_prefix",
            format!("id {value:?} must start with {prefix:?}"),
        )
    })?;
    if body.is_empty() {
        return Err(CoreError::new(
            "invalid_id_empty",
            format!("id {value:?} has no body after prefix"),
        ));
    }
    if body.len() > MAX_ID_BODY_LEN {
        return Err(CoreError::new(
            "invalid_id_length",
            format!("id body exceeds {MAX_ID_BODY_LEN} characters"),
        ));
    }
    if let Some(ch) = body.chars().find(|ch| !VALID_BODY_CHARS.contains(ch)) {
        return Err(CoreError::new(
            "invalid_id_chars",
            format!("id {value:?} contains disallowed character {ch:?}"),
        ));
    }
    Ok(())
}

pub fn validate_org_id(value: &str) -> Result<(), CoreError> {
    validate_id(ORG_ID_PREFIX, value)
}
pub fn validate_node_id(value: &str) -> Result<(), CoreError> {
    validate_id(NODE_ID_PREFIX, value)
}
pub fn validate_version_id(value: &str) -> Result<(), CoreError> {
    validate_id(VERSION_ID_PREFIX, value)
}
pub fn validate_user_id(value: &str) -> Result<(), CoreError> {
    validate_id(USER_ID_PREFIX, value)
}
pub fn validate_device_id(value: &str) -> Result<(), CoreError> {
    validate_id(DEVICE_ID_PREFIX, value)
}

/// Generate a process-unique ID for `prefix`. Not cryptographic; suitable for
/// local generation. Server-side generation should use its own deterministic
/// generator (see `crates/server`); this is the shared client/daemon path.
pub fn generate_id(prefix: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    // Hex lowercases naturally and stays within VALID_BODY_CHARS.
    format!("{prefix}{nanos:x}{sequence:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_ids() {
        validate_id(NODE_ID_PREFIX, "node_abc123").unwrap();
        validate_id(VERSION_ID_PREFIX, "ver_1").unwrap();
        validate_node_id("node_deadbeef").unwrap();
    }

    #[test]
    fn rejects_wrong_prefix_empty_body_and_bad_chars() {
        assert_eq!(
            validate_node_id("ver_abc").unwrap_err().code,
            "invalid_id_prefix"
        );
        assert_eq!(
            validate_id(NODE_ID_PREFIX, "node_").unwrap_err().code,
            "invalid_id_empty"
        );
        assert_eq!(
            validate_id(NODE_ID_PREFIX, "node_AB").unwrap_err().code,
            "invalid_id_chars"
        );
        assert_eq!(
            validate_id(NODE_ID_PREFIX, "node_ with space")
                .unwrap_err()
                .code,
            "invalid_id_chars"
        );
    }

    #[test]
    fn rejects_oversized_body() {
        let body = "a".repeat(MAX_ID_BODY_LEN + 1);
        let value = format!("{NODE_ID_PREFIX}{body}");
        assert_eq!(
            validate_id(NODE_ID_PREFIX, &value).unwrap_err().code,
            "invalid_id_length"
        );
    }

    #[test]
    fn generated_ids_are_validatable_and_unique() {
        let a = generate_id(NODE_ID_PREFIX);
        let b = generate_id(NODE_ID_PREFIX);
        validate_node_id(&a).unwrap();
        validate_node_id(&b).unwrap();
        assert_ne!(a, b);
    }
}
