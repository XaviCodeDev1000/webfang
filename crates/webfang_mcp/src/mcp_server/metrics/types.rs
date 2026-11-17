use serde::Serialize;
use std::time::Duration;

/// Outcome bucket for a scrape event (A1: success/error, NOT raw HTTP codes).
///
/// `#[non_exhaustive]` (issue #1614): this set is not complete by design — it
/// has grown once per new outcome concept (admission control, #1611, adds its
/// own dispositions), so it is a growing vocabulary. The attribute keeps the
/// next bucket an S1 / minor change instead of an S2 / major one, and it is
/// free here: the only exhaustive matches are in this crate (`ScrapeMetrics::
/// record`, `DefaultOverrides` derivation), and no test crate matches on it,
/// so `non_exhaustive` binds nobody. See
/// `docs/src/mcp-public-surface-policy.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Outcome {
    /// The underlying scraping operation returned `Ok`.
    Success,
    /// The underlying scraping operation returned `Err`.
    Error,
    /// Partial success — some URLs succeeded, some failed (batch operations).
    Partial,
}

impl Outcome {
    /// Whether this outcome is a full success.
    #[must_use]
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    /// Whether this outcome is partial (some successes, some failures).
    #[must_use]
    pub fn is_partial(self) -> bool {
        matches!(self, Self::Partial)
    }
}

/// One recorded scrape event.
#[derive(Debug, Clone)]
pub struct ScrapeEvent {
    /// Handler/tool name literal (e.g. `"scrape_url"`).
    pub tool: &'static str,
    /// Target host (`host_str()`) or `"unknown"` when unparseable (REQ-01).
    pub domain: String,
    /// Success/error bucket (A1).
    pub outcome: Outcome,
    /// Page/result count per the per-tool mapping.
    pub count: usize,
    /// Wall-clock duration of the operation.
    pub duration: Duration,
    /// Run-trace UUID (hex, dashed) of the tool call that produced this event;
    /// `Some` for handlers that mint a run-root identity (#698), `None` when
    /// the operation has no identity (e.g. synthetic test events).
    pub trace_id: Option<String>,
    /// W3C traceparent of the tool call's run-root identity (#698); pairs with
    /// `trace_id` so the metric event is reconstructable with the run trace.
    pub correlation_id: Option<String>,
}

impl ScrapeEvent {
    /// Build a scrape event without run identity (test / synthetic events).
    #[must_use]
    pub fn new(
        tool: &'static str,
        domain: String,
        outcome: Outcome,
        count: usize,
        duration: Duration,
    ) -> Self {
        Self {
            tool,
            domain,
            outcome,
            count,
            duration,
            trace_id: None,
            correlation_id: None,
        }
    }

    /// Build a scrape event stamped with the tool call's run-root identity
    /// (#698): the run-trace UUID as `trace_id` plus the W3C traceparent as
    /// `correlation_id`, so the metric event is reconstructable with the run
    /// trace. An MCP tool call IS an operation (#501) — handlers mint one
    /// identity at entry and share it across the call's success/error events
    /// through this single constructor, which is the only place that wires a
    /// [`CorrelationId`](webfang_core::domain::CorrelationId) into a
    /// [`ScrapeEvent`].
    #[must_use]
    pub fn identified(
        tool: &'static str,
        domain: String,
        outcome: Outcome,
        count: usize,
        duration: Duration,
        correlation: &webfang_core::domain::CorrelationId,
    ) -> Self {
        let mut event = Self::new(tool, domain, outcome, count, duration);
        event.trace_id = Some(correlation.trace_id().to_string());
        event.correlation_id = Some(correlation.to_traceparent());
        event
    }
}

/// Maximum number of distinct domains tracked in the per-domain breakdown
/// (REQ-05). Beyond this cap, new domains aggregate into [`OVERFLOW_DOMAIN_KEY`].
///
/// #1130: the value moved to `webfang_core::domain::budget` — the single
/// source shared with the `DomainSessionPool` cap — and is re-exported here
/// so existing `metrics::MAX_TRACKED_DOMAINS` paths keep working.
pub use webfang_core::domain::budget::MAX_TRACKED_DOMAINS;

/// Overflow bucket key for domains recorded past [`MAX_TRACKED_DOMAINS`]
/// (REQ-05). A domain literally named `"otros"` merges into this same bucket —
/// documented, accepted.
pub const OVERFLOW_DOMAIN_KEY: &str = "otros";

/// Per-domain aggregate (REQ-02).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DomainStats {
    /// Number of scrape events recorded for this domain.
    pub events: usize,
    /// Total pages/results accumulated across this domain's events.
    pub pages: usize,
    /// Number of error-outcome events for this domain.
    pub errors: usize,
}
