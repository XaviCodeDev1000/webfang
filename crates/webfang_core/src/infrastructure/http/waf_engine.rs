//! WAF Detection Engine - Layer 7 Protection
//!
//! This module provides advanced WAF detection beyond the basic signature matching
//! in http_client.rs. It includes:
//! - Detection by Control Headers (x-datadome-response, cf-mitigated, etc.)
//! - Entropy analysis for "Silent Challenge" detection
//! - Efficient O(N) matching using Aho-Corasick for 60+ signatures
//! - Context-aware, tiered, evidence-based inspection via [`WafInspector::inspect`]
//!
//! # Usage
//!
//! ```ignore
//! use webfang_core::domain::waf::InspectionContext;
//! use webfang_core::infrastructure::http::waf_engine::WafInspector;
//!
//! # let headers = wreq::header::HeaderMap::new();
//! # let body = String::new();
//! // Build the inspection context from the HTTP response (status + content-type
//! // + headers); callers with only the body use the degraded default.
//! let ctx = InspectionContext {
//!     status: Some(200),
//!     content_type: Some("text/html".into()),
//!     headers,
//!     ignore_waf: false,
//! };
//! let verdict = WafInspector::inspect(&body, &ctx);
//! if verdict.is_blocked {
//!     return Err(verdict.evidence_chain());
//! }
//! ```

// Internal modules (split by seam)
mod content_type;
mod entropy;
mod evidence;
mod inspector;
mod signatures;
mod verdict;

// Public re-exports — external callers use `waf_engine::` paths unchanged
pub use content_type::{is_html_content_type, should_scan_body};
pub use entropy::{calculate_entropy, entropy_evidence};
pub use evidence::{
    collect_body_evidence, collect_header_evidence, is_config_string_match, passes_boundary_filter,
};
pub use inspector::WafInspector;
pub use signatures::{
    supported_providers, BoundaryMode, ENTROPY_THRESHOLD, MAX_EVIDENCE_PER_BODY,
    SILENT_CHALLENGE_MAX_BYTES, SILENT_CHALLENGE_MIN_SCRIPTS, SUSPICIOUS_SIZE_THRESHOLD, WAF_AC,
    WAF_BODY_SIGNATURES, WAF_CONTROL_HEADERS,
};
pub use verdict::decide;

#[cfg(test)]
mod tests_common;
#[cfg(test)]
mod tests_context;
#[cfg(test)]
mod tests_degraded;
#[cfg(test)]
mod tests_detection;
#[cfg(test)]
mod tests_entropy;
#[cfg(test)]
mod tests_evidence;
#[cfg(test)]
mod tests_inspect;
#[cfg(test)]
mod tests_registry;

// Test-module wiring for a FILE-module parent: `#[path]` points at the
// sibling file explicitly while preserving the `waf_engine::entropy_policy`
// module path (the pattern `infrastructure/llm/client.rs` uses for
// `client_tests.rs`, issue #2614).
//
// It exists because `waf_engine.rs` is grandfathered in
// `scripts/quality-baselines.json` at 2570 lines and the size ratchet is
// DESCENDING: it may not grow by a single line, and the seven Miri gates this
// lane needs are seven lines. Extracting the rows SHRINKS this file by 151
// lines and leaves the other ninety tests of the module running under the
// interpreter. The header of the child file carries the measurement that
// justifies gating exactly these seven.
#[cfg(test)]
#[path = "waf_engine_tests.rs"]
mod entropy_policy;
