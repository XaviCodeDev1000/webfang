//! The resolver's resolution marker (#2784).
//!
//! A name that never comes back and a SYN that never gets through look the
//! same from the outside: both end in a timeout while the connection is being
//! established. The marker tells them apart: the resolver marks the host while
//! a resolution is in flight, and a failure that arrives while the mark is
//! still set is a **resolution** failure, not a connect one.
//!
//! The marker is per request: a resolution that belongs to ANOTHER request
//! leaves the cause undecided, and `unknown` is the honest answer.

use crate::domain::error::cause::NetworkCause;

/// The state of the name resolutions in flight for a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionState {
    /// No resolution is in flight for that host: the name resolved (or was
    /// never needed), so the failure is NOT a resolution failure.
    None,
    /// A resolution for that host started at or after this request began and
    /// has NOT finished: the name never came back, so the failure IS a
    /// resolution failure.
    ThisRequest,
    /// A resolution for that host is in flight, but it started BEFORE this
    /// request: it belongs to another request, so this failure cannot be
    /// attributed — the honest answer is `unknown`.
    OtherRequest,
}

/// Tracks the name resolutions in flight, per host.
///
/// The resolver marks a host as "resolving" when it starts and clears the mark
/// when it finishes. A failure that arrives while a mark is still set is a
/// resolution that never came back (#2784).
#[derive(Debug, Default)]
pub struct ResolutionTracker {
    in_flight: std::sync::Mutex<std::collections::HashMap<String, Vec<std::time::Instant>>>,
}

impl ResolutionTracker {
    /// Mark a resolution as started for `host`.
    ///
    /// The returned guard clears the mark when it is dropped, so a cancelled
    /// resolution never leaves a phantom mark behind.
    pub fn begin(&self, host: &str) -> ResolutionGuard<'_> {
        let started = std::time::Instant::now();
        if let Ok(mut marks) = self.in_flight.lock() {
            marks.entry(host.to_owned()).or_default().push(started);
        }
        ResolutionGuard {
            tracker: self,
            host: host.to_owned(),
            started,
        }
    }

    /// The state of the resolutions in flight for `host`.
    ///
    /// `request_started` is when the request that failed began: a resolution
    /// that started at or after it belongs to that request.
    #[must_use]
    pub fn state(&self, host: &str, request_started: std::time::Instant) -> ResolutionState {
        let Ok(marks) = self.in_flight.lock() else {
            return ResolutionState::None;
        };
        let Some(starts) = marks.get(host) else {
            return ResolutionState::None;
        };
        if starts.iter().any(|started| *started >= request_started) {
            ResolutionState::ThisRequest
        } else {
            ResolutionState::OtherRequest
        }
    }
}

/// Clears a resolution mark when the resolution ends.
#[derive(Debug)]
pub struct ResolutionGuard<'a> {
    tracker: &'a ResolutionTracker,
    host: String,
    started: std::time::Instant,
}

impl Drop for ResolutionGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut marks) = self.tracker.in_flight.lock() {
            if let Some(starts) = marks.get_mut(&self.host) {
                starts.retain(|started| *started != self.started);
                if starts.is_empty() {
                    marks.remove(&self.host);
                }
            }
        }
    }
}

/// The process-wide tracker of the resolutions in flight.
///
/// The resolver marks it and the failure sites read it, so both sides agree on
/// WHICH request a hung resolution belongs to (#2784).
pub fn resolution_tracker() -> &'static ResolutionTracker {
    static TRACKER: std::sync::OnceLock<ResolutionTracker> = std::sync::OnceLock::new();
    TRACKER.get_or_init(ResolutionTracker::default)
}

/// Refine a connect-phase cause with what the resolver knows.
///
/// A timeout while the name is still being resolved is a RESOLUTION failure,
/// not a connect one: the SYN was never sent. A resolution that belongs to
/// another request leaves the cause undecided, and `unknown` is the honest
/// answer (#2784).
#[must_use]
pub fn refine_connect_cause(
    cause: NetworkCause,
    host: &str,
    request_started: std::time::Instant,
) -> NetworkCause {
    if !matches!(cause, NetworkCause::ConnectTimeout) {
        return cause;
    }
    match resolution_tracker().state(host, request_started) {
        ResolutionState::ThisRequest => NetworkCause::DnsResolution,
        ResolutionState::OtherRequest => NetworkCause::Unknown,
        ResolutionState::None => cause,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error::cause::NetworkCause;
    use std::time::Instant;

    /// A connect timeout while THIS request's resolution is still in flight is
    /// a RESOLUTION failure: the SYN was never sent (#2784).
    #[test]
    fn a_hung_resolution_is_not_a_connect_timeout() {
        let tracker = ResolutionTracker::default();
        let request_started = Instant::now();
        let _mark = tracker.begin("example.test");
        assert_eq!(
            tracker.state("example.test", request_started),
            ResolutionState::ThisRequest
        );
    }

    /// A resolution that belongs to ANOTHER request leaves the cause
    /// undecided: `unknown` is the honest answer, not a false attribution.
    #[test]
    fn a_resolution_from_another_request_is_not_attributed() {
        let tracker = ResolutionTracker::default();
        let _mark = tracker.begin("example.test");
        let later = Instant::now() + std::time::Duration::from_secs(1);
        assert_eq!(
            tracker.state("example.test", later),
            ResolutionState::OtherRequest
        );
    }

    /// A finished resolution leaves no mark: the failure was NOT a resolution
    /// failure, so the connect cause stands.
    #[test]
    fn a_finished_resolution_leaves_no_mark() {
        let tracker = ResolutionTracker::default();
        {
            let _mark = tracker.begin("example.test");
        }
        assert_eq!(
            tracker.state("example.test", Instant::now()),
            ResolutionState::None
        );
    }

    /// The refinement: the connect timeout becomes the resolution failure when
    /// the marker says so, and stays `unknown` when it cannot decide.
    #[test]
    fn the_refinement_turns_a_connect_timeout_into_a_dns_failure() {
        let tracker = resolution_tracker();
        let request_started = Instant::now();
        let _mark = tracker.begin("webfang-taxonomy-probe.test");
        assert_eq!(
            refine_connect_cause(
                NetworkCause::ConnectTimeout,
                "webfang-taxonomy-probe.test",
                request_started
            ),
            NetworkCause::DnsResolution
        );
        // A cause that is not a timeout is never rewritten.
        assert_eq!(
            refine_connect_cause(
                NetworkCause::ConnectRefused,
                "webfang-taxonomy-probe.test",
                request_started
            ),
            NetworkCause::ConnectRefused
        );
    }
}
