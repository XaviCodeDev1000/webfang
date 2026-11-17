//! Scrape metrics — process-lifetime accumulator for MCP scraping activity.
//!
//! Pure types + logic only (DD-2): no DI, no locking, fully unit-testable.
//! Locking/tracing wiring lives on `McpState` (see `state.rs`). One structured
//! tracing event is emitted per recorded scrape (REQ-09) INSIDE
//! [`ScrapeMetrics::record`] (DD-3), keeping the critical section synchronous.
//!
//! Serialization is snapshot-safe (REQ-08): domains/tools use [`BTreeMap`] for
//! deterministic key order (DD-6) and the mean stays a single flat
//! `average_duration_ms` field (DD-5) that is trivially redactable.
//!
//! # SLO answerability (issue #1610 slice 4)
//!
//! `average_duration_ms` alone cannot answer a percentile question — a mean
//! destroys the distribution it summarizes, so p50/p95/p99 are not functions
//! of it. Two additive members close that gap, and both are *in-process
//! summaries* of a record the operator already owns in full: the structured
//! `scrape recorded` event emitted inside [`ScrapeMetrics::record`] is the
//! lossless record, the snapshot is the bounded index over it.
//!
//! * [`DurationReservoir`] (OBS-M4) keeps the last [`DURATION_SAMPLE_CAPACITY`]
//!   millisecond samples in a ring buffer and publishes p50/p95/p99 by
//!   nearest-rank.
//! * [`ToolDomainStats`] (OBS-M6) adds the tool x domain cross-product, which
//!   is what "which tool is slow for this domain" needs.
//!
//! The percentiles are reproducible from the trace file with one `jq` pass —
//! see [`DurationReservoir`] for the query and the exact nearest-rank rule.

use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;

mod distribution;
#[cfg(test)]
mod tests_core;
#[cfg(test)]
mod tests_percentiles;
#[cfg(test)]
mod tests_serialization;
#[cfg(test)]
mod tests_support;
#[cfg(test)]
mod tests_tool_domain;
#[cfg(test)]
mod tests_tracing;
mod tool_domain;
mod types;

pub use distribution::{DurationPercentiles, DurationReservoir, DURATION_SAMPLE_CAPACITY};
pub use tool_domain::{
    ToolDomainStats, MAX_TRACKED_TOOL_DOMAIN_PAIRS, TOOL_DOMAIN_DURATION_CAPACITY,
};
pub use types::{DomainStats, Outcome, ScrapeEvent, MAX_TRACKED_DOMAINS, OVERFLOW_DOMAIN_KEY};

/// Accumulator-side counters for one (tool, domain) cell (OBS-M6).
///
/// Private: the published shape is [`ToolDomainStats`], which adds the
/// reservoir projection. Kept separate so the hot path mutates three integers
/// while the reservoir lives in its own ring.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ToolDomainCounters {
    events: usize,
    pages: usize,
    errors: usize,
}

impl ToolDomainStats {
    /// Join one cell's counters with the percentiles of its reservoir.
    #[must_use]
    fn new(counters: &ToolDomainCounters, duration_percentiles: DurationPercentiles) -> Self {
        Self {
            events: counters.events,
            pages: counters.pages,
            errors: counters.errors,
            duration_percentiles,
        }
    }
}

/// Process-lifetime scrape accumulator (A3: no reset).
///
/// NOT `Serialize` — [`ScrapeMetrics::snapshot`] is the serializable view.
#[derive(Debug, Clone)]
pub struct ScrapeMetrics {
    total_events: usize,
    success_count: usize,
    error_count: usize,
    /// Partial-success events (batch with some failures).
    partial_count: usize,
    /// Per-domain breakdown; `BTreeMap` for deterministic order (DD-6).
    domains: BTreeMap<String, DomainStats>,
    /// Per-tool event counts; `BTreeMap` for deterministic order (DD-6).
    tools: BTreeMap<String, usize>,
    /// OBS-M6: tool x domain cross-product, `BTreeMap` on BOTH axes so the
    /// published key order is total and never hash-dependent.
    tool_domains: BTreeMap<String, BTreeMap<String, ToolDomainCounters>>,
    /// OBS-M6: per-pair duration reservoirs, in the SAME key space as
    /// `tool_domains` by construction. They live apart from the counters
    /// because a reservoir is mutable ring state and the published
    /// [`ToolDomainStats`] is an immutable projection of it — merging the two
    /// would mean either recomputing percentiles on every record (O(n log n)
    /// on the hot path) or serializing the ring itself (a large, order-stable
    /// but useless blob). The snapshot joins them once, at read time.
    tool_domain_durations: BTreeMap<String, BTreeMap<String, DurationReservoir>>,
    /// Live count of occupied cross-product cells, so the cap is a count and
    /// not a per-insert scan of the whole tree.
    tool_domain_pairs: usize,
    duration_sum: Duration,
    /// OBS-M4: bounded ring of the last [`DURATION_SAMPLE_CAPACITY`]
    /// millisecond samples. The SUM above is kept untouched: `average_duration_ms`
    /// remains a whole-lifetime mean and must not silently become a
    /// windowed one.
    durations: DurationReservoir,
}

impl Default for ScrapeMetrics {
    fn default() -> Self {
        Self {
            total_events: 0,
            success_count: 0,
            error_count: 0,
            partial_count: 0,
            domains: BTreeMap::new(),
            tools: BTreeMap::new(),
            tool_domains: BTreeMap::new(),
            tool_domain_durations: BTreeMap::new(),
            tool_domain_pairs: 0,
            duration_sum: Duration::ZERO,
            durations: DurationReservoir::with_capacity(DURATION_SAMPLE_CAPACITY),
        }
    }
}

impl ScrapeMetrics {
    /// Record a single scrape event.
    ///
    /// Short synchronous critical section: emits the structured tracing event
    /// (REQ-09) then mutates the aggregates. No `.await` here, so the lock held
    /// by the caller is never held across an await point (REQ-07). Every step
    /// is `O(1)` amortized or a single `BTreeMap` probe, so the section stays
    /// short even as the accumulators fill.
    pub fn record(&mut self, event: ScrapeEvent) {
        // The integer the event publishes is the integer the reservoirs keep
        // (OBS-M4). Computed once so the JSONL and the snapshot cannot drift.
        let duration_ms = event.duration.as_millis() as u64;
        tracing::info!(
            tool = %event.tool,
            domain = %event.domain,
            success = event.outcome.is_success(),
            duration_ms,
            pages = event.count,
            trace_id = event.trace_id.clone(),
            correlation_id = event.correlation_id.clone(),
            "scrape recorded"
        );

        self.total_events += 1;
        match event.outcome {
            Outcome::Success => self.success_count += 1,
            Outcome::Error => self.error_count += 1,
            Outcome::Partial => self.partial_count += 1,
        }

        // REQ-05: cap the per-domain map. Totals above are updated
        // unconditionally; already-tracked domains keep their own key even
        // at the cap so re-aggregation stays exact.
        let domain_key = self.resolve_domain_key(&event.domain);
        {
            let domain = self.domains.entry(domain_key.clone()).or_default();
            domain.events += 1;
            domain.pages += event.count;
            if matches!(event.outcome, Outcome::Error | Outcome::Partial) {
                domain.errors += 1;
            }
        }

        *self.tools.entry(event.tool.to_string()).or_default() += 1;
        self.duration_sum += event.duration;
        self.durations.record(duration_ms);
        // OBS-M6: keyed on the ALREADY-resolved domain key, so the cross-product
        // can never name a domain the per-domain map is hiding.
        self.record_tool_domain(event.tool, &domain_key, &event, duration_ms);
    }

    /// Resolve the per-domain key for `domain` (REQ-05).
    ///
    /// Returns an owned `String` rather than a borrow: the caller needs to
    /// insert into `self.domains` next, which a borrow of `self.domains`
    /// would forbid. Costs one short-string clone per event, which the
    /// per-domain `entry` allocation already pays.
    fn resolve_domain_key(&self, domain: &str) -> String {
        if self.domains.contains_key(domain) || self.domains.len() < MAX_TRACKED_DOMAINS {
            domain.to_string()
        } else {
            OVERFLOW_DOMAIN_KEY.to_string()
        }
    }

    /// Fold one event into the tool x domain cross-product (OBS-M6).
    ///
    /// Split from [`record`](Self::record) to keep both readable: `record` is
    /// the hot path and this is where the cross-product's two caps live.
    fn record_tool_domain(
        &mut self,
        tool: &str,
        domain_key: &str,
        event: &ScrapeEvent,
        duration_ms: u64,
    ) {
        let key = self.resolve_tool_domain_key(tool, domain_key);
        // Count the cell BEFORE the mutable borrows, so `tool_domain_pairs`
        // and the maps are not aliased.
        let is_new_cell = !self
            .tool_domains
            .get(tool)
            .is_some_and(|per_tool| per_tool.contains_key(&key));
        if is_new_cell {
            self.tool_domain_pairs += 1;
        }
        let counters = self
            .tool_domains
            .entry(tool.to_string())
            .or_default()
            .entry(key.clone())
            .or_default();
        counters.events += 1;
        counters.pages += event.count;
        // Same rule as the per-domain map (Partial counts as an error here);
        // splitting the two is OBS-M5 and deliberately NOT done in this slice.
        if matches!(event.outcome, Outcome::Error | Outcome::Partial) {
            counters.errors += 1;
        }
        self.tool_domain_durations
            .entry(tool.to_string())
            .or_default()
            .entry(key)
            .or_insert_with(|| DurationReservoir::with_capacity(TOOL_DOMAIN_DURATION_CAPACITY))
            .record(duration_ms);
    }

    /// Resolve the cross-product's domain key for `(tool, domain_key)`
    /// (OBS-M6).
    ///
    /// Two independent reasons to fold into [`OVERFLOW_DOMAIN_KEY`], checked
    /// in the same precedence the per-domain map uses (an already-tracked cell
    /// always wins, so re-aggregation stays exact):
    ///
    /// 1. the shared per-domain cap, applied upstream by
    ///    [`resolve_domain_key`](Self::resolve_domain_key); and
    /// 2. this slice's own [`MAX_TRACKED_TOOL_DOMAIN_PAIRS`] cap, which the
    ///    per-domain map has no reason to carry because it is not multiplied
    ///    by the tool count.
    fn resolve_tool_domain_key(&self, tool: &str, domain_key: &str) -> String {
        let already_tracked = self
            .tool_domains
            .get(tool)
            .is_some_and(|per_tool| per_tool.contains_key(domain_key));
        if already_tracked || self.tool_domain_pairs < MAX_TRACKED_TOOL_DOMAIN_PAIRS {
            domain_key.to_string()
        } else {
            OVERFLOW_DOMAIN_KEY.to_string()
        }
    }

    /// Whether any events have been recorded (DD-12: empty ≡ `total_events == 0`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total_events == 0
    }

    /// Produce a serializable point-in-time view (REQ-08).
    #[must_use]
    pub fn snapshot(&self) -> MetricsSnapshot {
        let average_duration_ms = if self.total_events == 0 {
            0.0
        } else {
            self.duration_sum.as_millis() as f64 / self.total_events as f64
        };
        MetricsSnapshot {
            total_events: self.total_events,
            success_count: self.success_count,
            error_count: self.error_count,
            partial_count: self.partial_count,
            domains: self.domains.clone(),
            tools: self.tools.clone(),
            average_duration_ms,
            duration_percentiles: self.durations.percentiles(),
            tool_domains: self.project_tool_domains(),
        }
    }

    /// Join the cross-product's counters with their reservoirs into the
    /// published view (OBS-M6).
    ///
    /// Walks the counter map, not the reservoir map, so a cell is emitted
    /// exactly when it exists — the two are written together in
    /// [`record_tool_domain`](Self::record_tool_domain) and a missing
    /// reservoir would be a bug, but a missing cell would be a lie in the
    /// snapshot. A cell with no reservoir yields empty percentiles (`None`
    /// ranks, `samples: 0`), which is the honest reading rather than a
    /// fabricated `0 ms`.
    fn project_tool_domains(&self) -> BTreeMap<String, BTreeMap<String, ToolDomainStats>> {
        self.tool_domains
            .iter()
            .map(|(tool, per_tool)| {
                let projected = per_tool
                    .iter()
                    .map(|(domain, counters)| {
                        let percentiles = self
                            .tool_domain_durations
                            .get(tool)
                            .and_then(|per_tool| per_tool.get(domain))
                            .map_or_else(
                                DurationPercentiles::empty,
                                DurationReservoir::percentiles,
                            );
                        (domain.clone(), ToolDomainStats::new(counters, percentiles))
                    })
                    .collect();
                (tool.clone(), projected)
            })
            .collect()
    }
}

/// Serializable point-in-time view of the accumulator (REQ-08).
///
/// The mean stays one flat `average_duration_ms` field (DD-5) so snapshots
/// stay deterministic and trivially redactable. The two OBS-M4/OBS-M6 members
/// are appended AFTER it: they are purely additive, so every field that
/// existed before keeps both its exact bytes and its exact position
/// (`pre_existing_snapshot_fields_are_byte_identical` pins that).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MetricsSnapshot {
    /// Total recorded scrape events.
    pub total_events: usize,
    /// Number of success-outcome events.
    pub success_count: usize,
    /// Number of error-outcome events.
    pub error_count: usize,
    /// Number of partial-success events (batch with some failures).
    pub partial_count: usize,
    /// Per-domain breakdown (sorted keys).
    pub domains: BTreeMap<String, DomainStats>,
    /// Per-tool event counts (sorted keys).
    pub tools: BTreeMap<String, usize>,
    /// Mean wall-clock duration in milliseconds across ALL events — a
    /// whole-lifetime mean, deliberately NOT windowed by
    /// [`duration_percentiles`](Self::duration_percentiles) even after that
    /// ring rolls over. A reader comparing the two is looking at "mean over
    /// the process" versus "median over the recent tail", which is a
    /// difference of scope, not a bug.
    pub average_duration_ms: f64,
    /// OBS-M4: p50/p95/p99 plus the sample-count metadata that says whether
    /// they describe the whole lifetime or the recent window. `None` when no
    /// scrape has been recorded.
    pub duration_percentiles: DurationPercentiles,
    /// OBS-M6: tool x domain cross-product, sorted on both axes. The domain
    /// axis is drawn from the SAME key space as [`domains`](Self::domains) —
    /// a domain beyond [`MAX_TRACKED_DOMAINS`] appears here only under
    /// [`OVERFLOW_DOMAIN_KEY`], never as a key of its own. The pair count is
    /// separately bounded by [`MAX_TRACKED_TOOL_DOMAIN_PAIRS`], past which new
    /// pairs fold into `(tool, "otros")`.
    pub tool_domains: BTreeMap<String, BTreeMap<String, ToolDomainStats>>,
}

/// Extract the target host from a URL, falling back to `"unknown"` (REQ-01).
#[must_use]
pub fn domain_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}
