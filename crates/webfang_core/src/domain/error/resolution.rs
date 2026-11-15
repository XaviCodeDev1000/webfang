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
    marks: std::sync::Mutex<std::collections::HashMap<String, HostResolutions>>,
}

/// What the tracker knows about one host.
#[derive(Debug, Default)]
struct HostResolutions {
    /// The resolutions that started and NEVER COMPLETED, with the instant each
    /// one started.
    ///
    /// A resolution that was CANCELLED — the client gave up waiting for it —
    /// stays here: that abandonment IS the evidence that the name never came
    /// back. A resolution that completed (answer or error) is removed.
    unfinished: Vec<std::time::Instant>,
    /// When the last resolution for this host completed SUCCESSFULLY.
    ///
    /// It is what tells two requests for the same host apart: if a resolution
    /// finished after this request began, that request's name came back.
    last_success: Option<std::time::Instant>,
}

impl ResolutionTracker {
    /// Mark a resolution as started for `host`.
    ///
    /// The returned guard reports the resolution's end. A resolution that
    /// COMPLETES leaves the tracker; one that was ABANDONED stays in it, since
    /// the abandonment is the evidence (#2784).
    pub fn begin(&self, host: &str) -> ResolutionGuard<'_> {
        let started = std::time::Instant::now();
        if let Ok(mut marks) = self.marks.lock() {
            let host_marks = marks.entry(host.to_owned()).or_default();
            host_marks.unfinished.push(started);
            // Bounded: only the most recent unfinished resolutions are evidence
            // worth keeping.
            if host_marks.unfinished.len() > 16 {
                let excess = host_marks.unfinished.len() - 16;
                host_marks.unfinished.drain(..excess);
            }
        }
        ResolutionGuard {
            tracker: self,
            host: host.to_owned(),
            started,
            completed: false,
        }
    }

    /// The state of the resolutions for `host`, from this request's point of view.
    ///
    /// `request_started` is when the request that failed began.
    ///
    /// The rule is deliberately CONSERVATIVE: a hung resolution is only
    /// attributed to this request when nothing proves otherwise.
    ///
    /// - no resolution in flight: the name resolved, so the failure is not a
    ///   resolution failure;
    /// - a resolution in flight that started at or after this request began,
    ///   AND no resolution for that host completed successfully after this
    ///   request began: this request's own name never came back;
    /// - anything else: the evidence does not decide, and `unknown` is the
    ///   honest answer.
    ///
    /// The third case is the one concurrency creates: request A resolves and
    /// its SYN is dropped while request B (started later) hangs its
    /// resolution — B's mark started after A began, so without the successful
    /// completion A would be labelled a resolution failure. With it, A is
    /// `unknown`: never a wrong attribution.
    #[must_use]
    pub fn state(&self, host: &str, request_started: std::time::Instant) -> ResolutionState {
        let Ok(marks) = self.marks.lock() else {
            return ResolutionState::None;
        };
        let Some(host_marks) = marks.get(host) else {
            return ResolutionState::None;
        };
        if host_marks.unfinished.is_empty() {
            return ResolutionState::None;
        }
        let completed_after = host_marks
            .last_success
            .is_some_and(|finished| finished >= request_started);
        let started_after = host_marks
            .unfinished
            .iter()
            .any(|started| *started >= request_started);
        match (completed_after, started_after) {
            (false, true) => ResolutionState::ThisRequest,
            _ => ResolutionState::OtherRequest,
        }
    }
}

/// Tracks one resolution, from the moment it starts to the moment it ends.
#[derive(Debug)]
pub struct ResolutionGuard<'a> {
    tracker: &'a ResolutionTracker,
    host: String,
    started: std::time::Instant,
    completed: bool,
}

impl ResolutionGuard<'_> {
    /// Record that the resolution COMPLETED, successfully or not.
    ///
    /// A completed resolution is not evidence of a hang, so it leaves the
    /// tracker. A successful one also records WHEN the name came back, which
    /// is what separates two requests for the same host (#2784).
    pub fn finished(&mut self, succeeded: bool) {
        self.completed = true;
        if let Ok(mut marks) = self.tracker.marks.lock() {
            if let Some(host_marks) = marks.get_mut(&self.host) {
                host_marks
                    .unfinished
                    .retain(|started| *started != self.started);
                if succeeded {
                    host_marks.last_success = Some(std::time::Instant::now());
                }
            }
        }
    }
}

impl Drop for ResolutionGuard<'_> {
    fn drop(&mut self) {
        // Nothing to do: a resolution that COMPLETED already left the tracker
        // in `finished`, and one that was ABANDONED (the client gave up waiting
        // for the name) leaves its mark behind on purpose — that abandonment is
        // exactly the evidence the marker exists for (#2784). The marks are
        // bounded by `begin()`.
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
    connect_phase: bool,
) -> NetworkCause {
    // Only the CONNECT phase can be a resolution failure, and only two causes
    // can be refined: a connect timeout, and an UNCLASSIFIED connect failure.
    // The second one matters: a hung resolver produces a connect deadline whose
    // chain carries no `io::Error` (the resolution never came back, so nothing
    // was ever wrapped) and wreq does not even call it a timeout — the
    // classifier alone cannot tell it from anything else. The marker can.
    if !connect_phase {
        return cause;
    }
    if !matches!(cause, NetworkCause::ConnectTimeout | NetworkCause::Unknown) {
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
            let mut mark = tracker.begin("example.test");
            mark.finished(true);
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
                request_started,
                true,
            ),
            NetworkCause::DnsResolution
        );
        // A cause that is not a timeout is never rewritten.
        assert_eq!(
            refine_connect_cause(
                NetworkCause::ConnectRefused,
                "webfang-taxonomy-probe.test",
                request_started,
                true,
            ),
            NetworkCause::ConnectRefused
        );
    }
}
