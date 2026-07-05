//! Daemon event stream seam (DAEMON_API.md "Event stream").
//!
//! The daemon exposes a one-way structured event stream first. This module
//! owns the `daemon.events.subscribe` ack payload and a helper to drain recent
//! buffered events. The actual NDJSON/SSE transport wiring is deferred — the
//! dev-loopback HTTP transport in `lib.rs` keeps its request/response shape,
//! and `daemon.events.subscribe` returns an ack describing how the future
//! stream will be delivered.
//!
//! Events themselves are produced by [`DaemonBackend::record_event`] into the
//! in-memory buffer. Production transports (Unix socket NDJSON, loopback SSE)
//! will drain that buffer through this module once wired.

use biohazardfs_api_types::ApiError;
use serde_json::Value;

use crate::backend::DaemonBackend;

/// Default maximum number of events the in-memory buffer holds before the
/// oldest are dropped. Clients must tolerate dropped events by resyncing
/// through state/list methods (DAEMON_API.md).
pub const EVENT_BUFFER_CAPACITY: usize = 1024;

/// Initial stable event family names advertised to subscribers. Mirrors
/// `biohazardfs_api_types::event_types` so schema introspection and the
/// subscribe ack share one source of truth.
pub fn known_event_types() -> &'static [&'static str] {
    use biohazardfs_api_types::event_types::*;
    &[
        DAEMON_STARTED,
        DAEMON_STOPPING,
        DAEMON_HEALTH_CHANGED,
        AUTH_CHANGED,
        MOUNT_ATTACHED,
        MOUNT_DETACHED,
        MOUNT_HEALTH_CHANGED,
        FILE_CHANGED,
        CACHE_STATE_CHANGED,
        CACHE_QUOTA_WARNING,
        TRANSFER_QUEUED,
        TRANSFER_PROGRESS,
        TRANSFER_COMPLETED,
        TRANSFER_FAILED,
        LOCK_CHANGED,
        CONFLICT_DETECTED,
        CONFLICT_RESOLVED,
        SNAPSHOT_CREATED,
        SNAPSHOT_MOUNTED,
        AUDIT_EVENT_RECORDED,
        WARNING_RAISED,
    ]
}

/// Build the ack payload for `daemon.events.subscribe`. Describes the future
/// NDJSON stream and returns a subscription id plus the recent replay window
/// so a caller can resync after a dropped stream.
pub fn subscribe_payload(backend: &DaemonBackend, params: &Value) -> Result<Value, ApiError> {
    // Optional filter: when set, the subscription would only deliver events
    // whose `type` matches. The transport is not wired yet, so this is recorded
    // as the subscriber's intent, not enforced here.
    let filter = params.get("filter").and_then(|v| v.as_str());
    let replay_limit = params
        .get("replay_limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(64) as usize;

    let events = backend.recent_events();
    let replay: Vec<Value> = events
        .iter()
        .rev()
        .take(replay_limit)
        .rev()
        .filter(|envelope| match filter {
            Some(wanted) => envelope.event_type == wanted,
            None => true,
        })
        .map(|envelope| {
            serde_json::to_value(envelope).unwrap_or_else(|_| {
                Value::String(format!("<unserializable event {}>", envelope.event_type))
            })
        })
        .collect();

    Ok(serde_json::json!({
        "subscription_id": format!("sub_{}", biohazardfs_api_types::request_id()),
        "transport": "ndjson_over_dev_loopback_http_stream",
        "state": "acknowledged",
        "note": "NDJSON stream transport is not yet wired; clients should drain via daemon.recent_events until the SSE/IPC stream is implemented",
        "schema_version": biohazardfs_api_types::EVENT_SCHEMA_VERSION,
        "filter": filter,
        "event_types": known_event_types(),
        "buffer_capacity": EVENT_BUFFER_CAPACITY,
        "replay": replay,
    }))
}

/// Drain recent events for a future transport. Bounded by `limit` so a slow
/// consumer cannot OOM the daemon; older events beyond the buffer are already
/// gone (clients resync through state/list).
pub fn drain_recent_events(backend: &DaemonBackend, limit: usize) -> Vec<Value> {
    let events = backend.recent_events();
    events
        .iter()
        .rev()
        .take(limit)
        .rev()
        .map(|envelope| {
            serde_json::to_value(envelope)
                .unwrap_or_else(|_| Value::String("<unserializable event>".to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use biohazardfs_api_types::Source;

    #[test]
    fn subscribe_returns_ack_with_replay_window() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        // The backend seeds a daemon.started event on construction.
        let payload = subscribe_payload(&backend, &serde_json::json!({})).unwrap();
        assert_eq!(payload["state"], "acknowledged");
        assert_eq!(payload["transport"], "ndjson_over_dev_loopback_http_stream");
        assert_eq!(
            payload["schema_version"],
            biohazardfs_api_types::EVENT_SCHEMA_VERSION
        );
        let replay = payload["replay"].as_array().unwrap();
        assert!(!replay.is_empty(), "seeded daemon.started should replay");
        assert!(replay.iter().any(|evt| evt["type"] == "daemon.started"));
    }

    #[test]
    fn subscribe_filter_restricts_replay() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        // Add a non-daemon.started event.
        backend.record_event(
            biohazardfs_api_types::event_types::CACHE_STATE_CHANGED,
            serde_json::json!({"node_id": "node_x"}),
        );
        let payload = subscribe_payload(
            &backend,
            &serde_json::json!({"filter": "cache.state_changed"}),
        )
        .unwrap();
        let replay = payload["replay"].as_array().unwrap();
        assert!(
            replay
                .iter()
                .all(|evt| evt["type"] == "cache.state_changed")
        );
        assert!(!replay.is_empty());
    }

    #[test]
    fn drain_recent_events_bounds_output() {
        let backend = DaemonBackend::new("127.0.0.1:47666");
        for index in 0..5 {
            backend.record_event(
                biohazardfs_api_types::event_types::WARNING_RAISED,
                serde_json::json!({"index": index}),
            );
        }
        let drained = drain_recent_events(&backend, 3);
        assert_eq!(drained.len(), 3);
        // Most recent three of the five warnings (after the seeded daemon.started).
        // Order is oldest-to-newest within the bounded slice.
        assert_eq!(
            drained.last().and_then(|v| v["data"]["index"].as_u64()),
            Some(4)
        );
    }

    #[test]
    fn known_event_types_are_unique_and_stable() {
        let types = known_event_types();
        let total = types.len();
        let mut sorted: Vec<&str> = types.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), total, "event type names must be unique");
        // Spot-check a few stable dotted names from the contract.
        assert!(types.contains(&"daemon.started"));
        assert!(types.contains(&"transfer.progress"));
        assert!(types.contains(&"conflict.resolved"));
    }

    #[test]
    fn source_filter_param_is_optional_and_ignored_safely() {
        let _ = Source::Test;
        let backend = DaemonBackend::new("127.0.0.1:47666");
        // Unknown extra params must not break the ack.
        let payload =
            subscribe_payload(&backend, &serde_json::json!({"bogus_param": "ignored"})).unwrap();
        assert_eq!(payload["state"], "acknowledged");
    }
}
