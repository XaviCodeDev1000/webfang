#![deny(missing_docs)]
// Tooling leaf exempt from the workspace panic lints (#2354): this crate
// compiles OUTSIDE cfg(test), so the `cfg_attr(test, ...)` carve-out used at
// the other roots does not apply here.
#![allow(clippy::unwrap_used, clippy::expect_used)]
//! WebFang Stage 0 benchmark harness (`webfang_benchmark`).
//!
//! Decision instrument, not a shipped feature: measures WebFang across cost
//! per 1k pages, success rate against WAF challenge patterns, latency p50/p95,
//! and SPA-heavy crawl behavior — entirely in-process over a local simulated
//! corpus (Tier A). Zero production-code dependencies beyond the public core
//! API; this crate is a workspace leaf.

pub mod aggregate;
pub mod competitor;
pub mod corpus;
pub mod cost;
pub mod error;
pub mod report;
pub mod runner;

pub use error::{BenchmarkError, Result};
