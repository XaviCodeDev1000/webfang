//! Scraping Core tools — 8 tools for URL scraping and crawling
//!
//! Tools: scrape_url, scrape_with_options, scrape_batch, crawl_site,
//! crawl_with_sitemap, discover_urls, discover_sitemap, detect_spa

use super::McpHandler;
use crate::mcp_server::metrics::{domain_of, Outcome};
use crate::mcp_server::params::*;
use crate::mcp_server::provenance;
use crate::mcp_server::validation::{
    invalid_params_with_reason, REASON_NOT_IN_ALLOWED_SET, REASON_UNSUPPORTED_SCHEME,
};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::tool;
use rmcp::tool_router;
use rmcp::{model::CallToolResult, ErrorData as McpError};
use safety::{filter_ssrf_safe, robots_denied_response};
use serialization::{
    batch_outcome_to_jsonl, discovered_urls_tool_result, record_batch_metrics_by_domain,
    scraped_results_to_provenance_jsonl,
};
use session_budget::{
    session_budget_message, within_session_budget, SessionResultOverflow, SESSION_RESULT_OPERATION,
};
use std::time::Instant;
use tracing::instrument;

/// Runtime-effective `crawl_site` default for `max_depth`, applied via
/// `unwrap_or` when the MCP parameter is omitted (#940 F1). The schema bridge
/// advertises THESE values — not the CLI/spec defaults (2/10) — so the
/// advertised schema stays truthful. Single source shared by the handler and
/// the bridge (and the parity tests); changing this constant moves both
/// together.
pub const CRAWL_SITE_DEFAULT_MAX_DEPTH: u8 = 3;

/// Runtime-effective `crawl_site` default for `max_pages`; see
/// [`CRAWL_SITE_DEFAULT_MAX_DEPTH`] (#940 F1).
pub const CRAWL_SITE_DEFAULT_MAX_PAGES: u32 = 100;

/// Runtime-effective `scrape_batch` default for `concurrency` (#1294 NS-04).
///
/// The handler applies it with `unwrap_or`, the schema bridge advertises it as the
/// property's `default`, and the contract suite compares it against
/// `ScraperConfig::default().scraper_concurrency` — the value the tool actually fell
/// back to before this constant existed, when the only announcement was a doc
/// comment claiming 4. Single source, same pattern as
/// [`CRAWL_SITE_DEFAULT_MAX_DEPTH`].
pub const SCRAPE_BATCH_DEFAULT_CONCURRENCY: usize = 3;

/// Build the scraping core router (8 tools for URL scraping and crawling).
///
/// Returns a partial router that is combined with the other category routers
/// via the `+` operator in
/// [`build_tool_router`](crate::mcp_server::handlers::build_tool_router).
pub fn build_router() -> ToolRouter<McpHandler> {
    let mut router = McpHandler::tool_router_crawl();
    router.merge(McpHandler::tool_router_discovery());
    router.merge(McpHandler::tool_router_scrape());
    router
}

mod safety;
mod serialization;
mod session_budget;
mod session_capture;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_batch_metrics;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_budget;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_common;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_crawl_sitemap;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_discovered_urls;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_discovery;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_robots;
#[cfg(all(test, not(miri)))] // wiremock + wreq use btls-sys FFI (unsupported by Miri)
mod tests_ssrf;
mod tools_crawl;
mod tools_discovery;
mod tools_scrape;
