// unwrap/expect are denied workspace-wide ([workspace.lints.clippy], #2354);
// this attribute is the test-only carve-out, and unlike the old per-crate
// `cfg_attr(not(test), deny)` copy it also covers this package's bins.
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
// Raw process-env mutations must go through webfang_test_utils (#1126,
// #1349). Ungated on purpose: cfg(test) mods in this crate are covered too.
#![deny(clippy::disallowed_methods)]
#![deny(missing_docs)]
#![deny(clippy::missing_errors_doc)]
#![deny(clippy::missing_panics_doc)]
// Explicit per docs-truthfulness audit (S5): these rustdoc lints are
// allow-by-default, so CI's `-D warnings` never reaches them. Declare intent.
#![allow(rustdoc::missing_crate_level_docs)]
#![allow(rustdoc::private_doc_tests)]
#![allow(rustdoc::unescaped_backticks)]
//! WebFang MCP — Model Context Protocol server
//!
//! Exposes scraper tools to AI agents via MCP protocol.
//! Depends on `webfang_core` for domain types.

pub mod mcp_server;
