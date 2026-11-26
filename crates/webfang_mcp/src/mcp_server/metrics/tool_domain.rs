use super::*;
use serde::Serialize;

/// Capacity of EACH tool x domain duration reservoir (OBS-M6).
///
/// Smaller than the global reservoir on purpose: the cross-product multiplies
/// the number of reservoirs by the number of tracked cells, so a per-cell cap
/// equal to the global one would make the bound quadratic in practice. 32
/// samples still pins p50 and p95 for a pair (the ranks land inside it for any
/// pair with >= 4 events) and keeps a full cross-product in the low hundreds of
/// kilobytes.
pub const TOOL_DOMAIN_DURATION_CAPACITY: usize = 32;

/// Maximum number of tracked tool x domain cells (OBS-M6).
///
/// The per-domain map is bounded by [`MAX_TRACKED_DOMAINS`], an ACCEPTED
/// residual this slice does not re-open. The cross-product is not bounded by
/// that alone: the tool count ([`crate::mcp_server::mcp_tool_count`])
/// x 500 domains is ~18 000 possible cells, ~26x the per-domain map, so it
/// needs its own stated bound. Past this many tracked cells a new
/// `(tool, domain)` pair folds into `(tool, [`OVERFLOW_DOMAIN_KEY`])`
/// — the same overflow convention, giving at most one overflow cell per tool.
pub const MAX_TRACKED_TOOL_DOMAIN_PAIRS: usize = 512;

/// Per (tool, domain) aggregate in the cross-product (OBS-M6).
///
/// The counters are deliberately the same three the per-domain map publishes
/// (and deliberately NOT re-classified: separating Partial from Error is
/// OBS-M5 and out of this slice), so a reader can move between the two views
/// without a translation table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ToolDomainStats {
    /// Number of events recorded for this tool on this domain.
    pub events: usize,
    /// Total pages/results accumulated across this pair's events.
    pub pages: usize,
    /// Number of events bucketed as Error or Partial (see OBS-M5).
    pub errors: usize,
    /// Percentiles over this pair's own bounded reservoir. This is what turns
    /// "which tool is slow for this domain" from a guess into a lookup: the
    /// per-domain map can say `a.com` got slow, and only this says WHICH tool.
    pub duration_percentiles: DurationPercentiles,
}
