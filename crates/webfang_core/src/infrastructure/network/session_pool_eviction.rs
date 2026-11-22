//! Bounded-eviction mechanics of the domain session pool (#1130).
//!
//! Split seam: TTL healing of expired sessions and the shared
//! `MAX_TRACKED_DOMAINS` cap scan. Both MUST run with no `sessions` shard
//! guard held (the RefMut-dropped-BEFORE-scan invariant); the orchestration
//! that calls them (`acquire`, `evict_stale`) stays in the parent module.

use std::time::Instant;

use tracing::debug;

use crate::domain::budget::MAX_TRACKED_DOMAINS;

use super::{DomainSessionPool, SessionState, SessionStatus};

impl DomainSessionPool {
    /// Reset sessions whose TTL has expired back to healthy.
    pub(super) fn evict_expired(&self, domain: &str, sessions: &mut [SessionState], now: Instant) {
        for state in sessions.iter_mut() {
            if let Some(last_failure) = state.last_failure_time {
                if now.duration_since(last_failure) > self.config.ttl_duration
                    && state.status != SessionStatus::Healthy
                {
                    debug!(domain, "evicting stale session (TTL expired)");
                    *state = SessionState::healthy();
                }
            }
        }
    }

    /// Soft cap on tracked domains (#1130): past the shared
    /// [`MAX_TRACKED_DOMAINS`] bound (same value the MCP metrics breakdown
    /// enforces, REQ-05), the least-recently-seen domains are removed
    /// outright so a long-lived server's map stays bounded.
    ///
    /// MUST be called with no `sessions` shard guard held — it scans and
    /// removes from the map (the same RefMut-dropped-BEFORE-scan invariant
    /// as `acquire`). In steady state the excess is 1: one O(cap) scan per
    /// insert past the cap, dominated by the network I/O it precedes.
    pub(super) fn enforce_domain_cap(&self) {
        let tracked = self.sessions.len();
        if tracked <= MAX_TRACKED_DOMAINS {
            return;
        }
        let excess = tracked - MAX_TRACKED_DOMAINS;
        let mut by_age: Vec<(String, Instant)> = self
            .sessions
            .iter()
            .map(|e| (e.key().clone(), e.value().last_seen))
            .collect();
        by_age.sort_by_key(|(_, last_seen)| *last_seen);
        for (domain, _) in by_age.into_iter().take(excess) {
            self.sessions.remove(&domain);
            debug!(
                domain = %domain,
                tracked = self.sessions.len(),
                cap = MAX_TRACKED_DOMAINS,
                "session-pool domain evicted (cap)"
            );
        }
    }
}
