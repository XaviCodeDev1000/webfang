// Test policy (#1951): tokio::spawn in tests is deliberate harness
// concurrency — blanket exemption; production keeps per-site supervision.
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! Pins the MCP tool surface as a reviewable contract (#3098, E14).
//!
//! The count-only pin in `stdio_handshake.rs` cannot see a rename (36 stays 36).
//! The recorded fixture in `public_surface_policy_test.rs` does compare names,
//! but it is regenerated with one command and never asserts a count. And a
//! route registered twice is invisible to both: rmcp's `ToolRouter` keeps
//! routes in a `HashMap` and a duplicate name silently replaces the earlier
//! route, so the merged surface still reports the same names and the same
//! count. (Two `#[tool]` functions cannot share a name — the macro generates
//! inherent methods and rustc rejects the ambiguity — so the duplicate seam is
//! the dynamic route merge, which is how `test_probe` registers.)
//!
//! These tests compare the LIVE registry against the hand-maintained list in
//! `tests/common/mcp_contract.rs` and name every offending tool.

#[path = "common/mcp_contract.rs"]
mod mcp_contract;

use std::collections::BTreeSet;
use std::fmt::Write as _;

use mcp_contract::{PINNED_TOOL_COUNT, PINNED_TOOL_NAMES};
use rmcp::handler::server::tool::ToolRouter;
use webfang_mcp::mcp_server::handlers;
use webfang_mcp::mcp_server::handlers::build_tool_router;
use webfang_mcp::mcp_server::McpHandler;

/// Mirrors `mcp_server::test_probe::PANIC_PROBE_TOOL_NAME`, which is `pub(crate)`.
/// `stdio_handshake.rs` spells the same literal for the same reason.
const PANIC_PROBE_TOOL_NAME: &str = "test_panic_probe";

/// The nine category routers `build_tool_router()` merges, in merge order.
///
/// They are built separately on purpose: the merged router hides a shadowed
/// route (last insert wins) while the category split names the router that
/// registered it twice — with the same names and the same count in both views.
fn category_routers() -> [(&'static str, ToolRouter<McpHandler>); 9] {
    [
        ("scraping", handlers::scraping::build_router()),
        ("content", handlers::content::build_router()),
        ("export", handlers::export::build_router()),
        ("url_utils", handlers::url_utils::build_router()),
        ("security", handlers::security::build_router()),
        ("obsidian", handlers::obsidian::build_router()),
        ("assets", handlers::assets::build_router()),
        ("ai", handlers::ai::build_router()),
        ("axtree", handlers::axtree::build_router()),
    ]
}

fn registered_names(router: &ToolRouter<McpHandler>) -> Vec<String> {
    router
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect()
}

fn pinned_set() -> BTreeSet<&'static str> {
    PINNED_TOOL_NAMES.iter().copied().collect()
}

#[test]
fn pinned_contract_lists_every_tool_exactly_once() {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut repeated: Vec<&str> = Vec::new();
    for &name in PINNED_TOOL_NAMES {
        if !seen.insert(name) {
            repeated.push(name);
        }
    }

    assert!(
        repeated.is_empty(),
        "the pinned contract lists a tool more than once: {} — \
         every tool gets exactly one entry in PINNED_TOOL_NAMES",
        repeated.join(", ")
    );
    assert_eq!(
        seen.len(),
        PINNED_TOOL_COUNT,
        "PINNED_TOOL_COUNT must derive from PINNED_TOOL_NAMES, not from a literal"
    );
}

#[test]
fn registered_tools_match_the_pinned_contract() {
    let registered = registered_names(&build_tool_router());
    let registered_set: BTreeSet<&str> = registered.iter().map(String::as_str).collect();
    let mut pinned = pinned_set();

    // The probe is development-only and env-gated: it may be registered in a
    // debug build that sets the variable, and it is not part of the contract.
    let probe_registered = registered_set.contains(PANIC_PROBE_TOOL_NAME);
    if probe_registered {
        pinned.insert(PANIC_PROBE_TOOL_NAME);
    }

    let mut report = String::new();
    for name in registered_set.difference(&pinned) {
        let _ = writeln!(
            report,
            "MCP tool added without updating the pinned contract: {name}"
        );
    }
    for name in pinned.difference(&registered_set) {
        let _ = writeln!(
            report,
            "MCP tool removed or renamed without updating the pinned contract: {name}"
        );
    }
    assert!(
        report.is_empty(),
        "the MCP surface moved (#3098): the contract lives at \
         crates/webfang_mcp/tests/common/mcp_contract.rs\n{report}"
    );

    let expected = PINNED_TOOL_COUNT + usize::from(probe_registered);
    assert_eq!(
        registered.len(),
        expected,
        "the registry exposes {} tools but the contract pins {PINNED_TOOL_COUNT}{}",
        registered.len(),
        if probe_registered {
            " plus the env-gated probe"
        } else {
            ""
        }
    );
}

#[test]
fn category_routers_partition_the_pinned_contract() {
    let pinned = pinned_set();
    let mut total = 0_usize;
    let mut union: BTreeSet<String> = BTreeSet::new();
    let mut duplicates: Vec<String> = Vec::new();

    for (category, router) in category_routers() {
        for name in registered_names(&router) {
            total += 1;
            if !union.insert(name.clone()) {
                duplicates.push(format!("{name} (re-registered by `{category}`)"));
            }
        }
    }

    assert!(
        duplicates.is_empty(),
        "two category routers register the same tool name, and rmcp keeps only \
         the last route silently: {}",
        duplicates.join(", ")
    );
    assert_eq!(
        total, PINNED_TOOL_COUNT,
        "the category routers register {total} routes but the contract pins {PINNED_TOOL_COUNT}"
    );

    let union_set: BTreeSet<&str> = union.iter().map(String::as_str).collect();
    let mut report = String::new();
    for name in union_set.difference(&pinned) {
        let _ = writeln!(report, "registered but not pinned: {name}");
    }
    for name in pinned.difference(&union_set) {
        let _ = writeln!(report, "pinned but not registered by any category: {name}");
    }
    assert!(
        report.is_empty(),
        "the category routers do not partition the pinned contract\n{report}"
    );
}
