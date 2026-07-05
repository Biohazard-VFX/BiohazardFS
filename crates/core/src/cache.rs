//! Cache state machine (FILESYSTEM_SEMANTICS.md).
//!
//! Defines the cache lifecycle states and the legal transitions between them,
//! backed by an in-memory map at the daemon layer. The critical invariant —
//! dirty and pinned entries are never auto-evicted — is enforced by
//! [`is_evictable`] and [`transition`]. Real eviction policy (LRU / size-based)
//! is a later hardening pass; this layer owns the states and the safety rules.

use crate::error::CoreError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheState {
    /// Not present in the local cache.
    Absent,
    /// User requested pinning; in flight.
    Pinning,
    /// Pinned: kept regardless of pressure. Never auto-evicted.
    Pinned,
    /// Hydration in progress (downloading from object store).
    Populating,
    /// Fully cached and readable. Evictable unless pinned/dirty.
    Ready,
    /// Local changes not yet committed to the server. Never evicted.
    Dirty,
    /// Eviction in progress.
    Evicting,
    /// Last hydrate/evict/verify failed; needs `cache.repair`.
    Failed,
}

impl CacheState {
    /// True only when the entry may be considered for eviction. Dirty and
    /// pinned entries are never evictable (FILESYSTEM_SEMANTICS.md invariant).
    pub fn is_evictable(self) -> bool {
        matches!(self, CacheState::Ready)
    }
}

/// Legal forward transitions. Anything not listed is rejected, so unsafe
/// moves (Dirty -> Evicting, Dirty -> Absent, Pinned -> Evicting) are
/// unrepresentable through [`transition`].
fn transition_allowed(from: CacheState, to: CacheState) -> bool {
    use CacheState::*;
    matches!(
        (from, to),
        (Absent, Pinning)
            | (Absent, Populating)
            | (Pinning, Pinned)
            | (Pinning, Failed)
            | (Pinned, Ready)
            | (Pinned, Populating)
            | (Populating, Ready)
            | (Populating, Pinned)
            | (Populating, Failed)
            | (Ready, Dirty)
            | (Ready, Evicting)
            | (Ready, Pinned)
            | (Ready, Populating)
            | (Dirty, Ready)
            | (Dirty, Failed)
            | (Evicting, Absent)
            | (Evicting, Failed)
            | (Evicting, Ready)
            | (Failed, Populating)
            | (Failed, Absent)
    )
}

/// Move from one cache state to another, rejecting illegal transitions.
/// `Dirty -> Ready` is allowed (upload acknowledged) but `Dirty -> Evicting` /
/// `Dirty -> Absent` are not: dirty data must never be lost.
pub fn transition(from: CacheState, to: CacheState) -> Result<CacheState, CoreError> {
    if from == to {
        return Ok(from);
    }
    if !transition_allowed(from, to) {
        return Err(CoreError::new(
            "illegal_cache_transition",
            format!(
                "cannot transition cache state {from:?} -> {to:?}; dirty and pinned entries are never auto-evicted"
            ),
        ));
    }
    Ok(to)
}

/// A cache entry's durable state. Held in-memory at the daemon; a future SQLite
/// store will persist a projection of these fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    pub node_id: String,
    pub version_id: Option<String>,
    pub state: CacheState,
    pub content_hash: Option<String>,
    pub size_bytes: u64,
    pub pinned: bool,
    pub dirty: bool,
    /// RFC3339 UTC of last read/write access; advisory for future eviction.
    pub last_accessed_at: Option<String>,
}

impl CacheEntry {
    /// Convenience: an entry counts as evictable only when Ready, not pinned,
    /// and not dirty. Belt-and-suspenders alongside [`CacheState::is_evictable`].
    pub fn is_evictable(&self) -> bool {
        self.state == CacheState::Ready && !self.pinned && !self.dirty
    }
}

/// Roll-up used by `cache.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    pub total_entries: u64,
    pub ready_entries: u64,
    pub pinned_entries: u64,
    pub dirty_entries: u64,
    pub failed_entries: u64,
    pub used_bytes: u64,
    pub pinned_bytes: u64,
    pub dirty_bytes: u64,
    pub quota_bytes: Option<u64>,
}

impl CacheStats {
    pub fn from_entries<'a>(
        entries: impl IntoIterator<Item = &'a CacheEntry>,
        quota_bytes: Option<u64>,
    ) -> Self {
        let mut stats = CacheStats {
            quota_bytes,
            ..Default::default()
        };
        for entry in entries {
            stats.total_entries += 1;
            stats.used_bytes += entry.size_bytes;
            match entry.state {
                CacheState::Ready | CacheState::Populating | CacheState::Pinned => {}
                CacheState::Dirty => {
                    stats.dirty_entries += 1;
                    stats.dirty_bytes += entry.size_bytes;
                }
                CacheState::Failed => {
                    stats.failed_entries += 1;
                }
                _ => {}
            }
            if entry.pinned {
                stats.pinned_entries += 1;
                stats.pinned_bytes += entry.size_bytes;
            }
            if entry.state == CacheState::Ready {
                stats.ready_entries += 1;
            }
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_hydrate_and_dirty_to_ready() {
        assert_eq!(
            transition(CacheState::Absent, CacheState::Populating).unwrap(),
            CacheState::Populating
        );
        assert_eq!(
            transition(CacheState::Populating, CacheState::Ready).unwrap(),
            CacheState::Ready
        );
        assert_eq!(
            transition(CacheState::Ready, CacheState::Dirty).unwrap(),
            CacheState::Dirty
        );
        assert_eq!(
            transition(CacheState::Dirty, CacheState::Ready).unwrap(),
            CacheState::Ready
        );
    }

    #[test]
    fn dirty_and_pinned_never_evict() {
        assert_eq!(
            transition(CacheState::Dirty, CacheState::Evicting)
                .unwrap_err()
                .code,
            "illegal_cache_transition"
        );
        assert_eq!(
            transition(CacheState::Dirty, CacheState::Absent)
                .unwrap_err()
                .code,
            "illegal_cache_transition"
        );
        assert_eq!(
            transition(CacheState::Pinned, CacheState::Evicting)
                .unwrap_err()
                .code,
            "illegal_cache_transition"
        );
    }

    #[test]
    fn entry_evictable_only_when_ready_unpinned_clean() {
        let mut entry = CacheEntry {
            node_id: "node_x".to_string(),
            version_id: None,
            state: CacheState::Ready,
            content_hash: None,
            size_bytes: 10,
            pinned: false,
            dirty: false,
            last_accessed_at: None,
        };
        assert!(entry.is_evictable());
        entry.dirty = true;
        assert!(!entry.is_evictable());
        entry.dirty = false;
        entry.pinned = true;
        assert!(!entry.is_evictable());
    }

    #[test]
    fn stats_count_dirty_and_pinned_separately() {
        let entries = vec![
            CacheEntry {
                node_id: "a".into(),
                version_id: None,
                state: CacheState::Ready,
                content_hash: None,
                size_bytes: 100,
                pinned: false,
                dirty: false,
                last_accessed_at: None,
            },
            CacheEntry {
                node_id: "b".into(),
                version_id: None,
                state: CacheState::Dirty,
                content_hash: None,
                size_bytes: 50,
                pinned: false,
                dirty: true,
                last_accessed_at: None,
            },
            CacheEntry {
                node_id: "c".into(),
                version_id: None,
                state: CacheState::Ready,
                content_hash: None,
                size_bytes: 25,
                pinned: true,
                dirty: false,
                last_accessed_at: None,
            },
        ];
        let stats = CacheStats::from_entries(&entries, Some(1000));
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.dirty_entries, 1);
        assert_eq!(stats.pinned_entries, 1);
        assert_eq!(stats.used_bytes, 175);
        assert_eq!(stats.dirty_bytes, 50);
        assert_eq!(stats.pinned_bytes, 25);
    }

    #[test]
    fn cache_state_serializes_snake_case() {
        let value = serde_json::to_value(CacheState::Populating).unwrap();
        assert_eq!(value, serde_json::Value::String("populating".to_string()));
    }
}
