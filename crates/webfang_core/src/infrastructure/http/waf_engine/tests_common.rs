//! Shared base for the `waf_engine` tests.
//!
//! Extracted from the inline `mod tests` of `waf_engine.rs` (PR 1 of the Ola 1
//! mission, umbrella #2584). `ctx_with` does not live with the verdict tests
//! despite having been born there: its consumers are three different children
//! (`tests_evidence` for the evidence cap, `tests_inspect` for the verdict policy
//! and `tests_entropy` for the entropy rows), so the destination is dictated by
//! the consumers and not by convenience. `pub(super)` = visible in the
//! `waf_engine` module and its descendants.

use crate::domain::waf::InspectionContext;

pub(super) fn ctx_with(status: Option<u16>, content_type: Option<&str>) -> InspectionContext {
    InspectionContext {
        status,
        content_type: content_type.map(String::from),
        ..Default::default()
    }
}
