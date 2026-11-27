//! The reviewed MCP tool contract (#3098, E14).
//!
//! This file is the ONE place a human audits the MCP surface: every tool name
//! `build_tool_router()` registers appears here exactly once, and the count is
//! derived from this list so the two cannot disagree.
//!
//! Targets include it with `#[path = "common/mcp_contract.rs"] mod mcp_contract;`
//! and each binary uses only a subset, so unused items are allowed here instead
//! of per item (the same reason `tests/common/mod.rs` carries the allow).
//!
//! Adding, removing or renaming a tool is a contract change: update this list
//! deliberately, in the same commit, and say why.
#![allow(dead_code)]

/// Every MCP tool name the registry exposes, sorted.
///
/// One literal per tool: a name may not appear twice. The panic probe
/// (`test_probe::PANIC_PROBE_TOOL_NAME`) is development-only and env-gated, so
/// it is deliberately NOT part of the contract.
///
/// A slice, not a fixed-size array, on purpose: an array would need its length
/// spelled a second time, and that second `36` is exactly the pin that can
/// disagree with the list.
pub const PINNED_TOOL_NAMES: &[&str] = &[
    "build_obsidian_uri",
    "clean_html",
    "convert_html_to_markdown",
    "convert_wiki_links",
    "crawl_site",
    "crawl_with_sitemap",
    "detect_obsidian_vault",
    "detect_spa",
    "detect_waf",
    "discover_sitemap",
    "discover_urls",
    "download_assets",
    "export_file",
    "export_jsonl",
    "export_vector",
    "extract_domain",
    "extract_links",
    "generate_frontmatter",
    "generate_rich_metadata",
    "get_accessibility_snapshot",
    "get_scrape_metrics",
    "highlight_code_blocks",
    "is_internal_link",
    "list_waf_providers",
    "match_url_pattern",
    "normalize_url",
    "open_in_obsidian",
    "process_export_pipeline",
    "scrape_batch",
    "scrape_url",
    "scrape_with_options",
    "search_obsidian",
    "semantic_cleaner",
    "url_to_file_path",
    "validate_url",
    "verify_waf_integrity",
];

/// The pinned tool count, derived from [`PINNED_TOOL_NAMES`].
///
/// Deriving is the point: a second literal `36` anywhere in the suite is a pin
/// that can disagree with the name list, which is exactly what E14 reported.
pub const PINNED_TOOL_COUNT: usize = PINNED_TOOL_NAMES.len();
