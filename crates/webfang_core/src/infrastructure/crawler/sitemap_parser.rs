//! Sitemap Parser Module
//!
//! Bounded buffered parser for XML sitemaps.
//! Supports gzip compression and sitemap index recursion.
//!
//! # Memory contract (issue #1822)
//!
//! The parser buffers, it does not stream: each response body is accumulated
//! up to `SitemapConfig::max_response_size`, decompressed up to
//! `SitemapConfig::max_decompressed_size`, and the aggregate URL set of a
//! sitemap index is held under `SitemapConfig::memory_limit_mb` (typed
//! [`SitemapError::MemoryLimitExceeded`] beyond it). Children of an index are
//! parsed with bounded concurrency and the combined result is deduplicated
//! and sorted.
//!
//! # Examples
//!
//! ```no_run
//! use webfang_core::domain::CorrelationId;
//! use webfang_core::infrastructure::crawler::SitemapParser;
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let parser = SitemapParser::new()?;
//! let urls = parser.parse_from_url("https://example.com/sitemap.xml", &CorrelationId::new()).await?;
//! println!("Found {} URLs", urls.len());
//! # Ok(())
//! # }
//! ```
//!
//! # Errors
//!
//! Returns `SitemapError` if:
//! - URL is invalid
//! - HTTP request fails
//! - XML parsing fails
//! - No `<loc>` elements found
//! - The aggregate URL set exceeds the configured memory limit

use super::batch_processor::BatchProcessor;
use super::compression_handler::CompressionHandler;
use super::memory_manager::{MemoryError, MemoryManager};
use super::retry_policy::RetryPolicy;
use super::url_validator::UrlValidator;
use crate::domain::crawler_port::sitemap::SitemapParserPort;
use crate::domain::crawler_port::SitemapConfig;
use crate::domain::url_validation::is_internal_link;
use crate::domain::waf::InspectionContext;
use crate::domain::{CorrelationId, CrawlError, UrlValidatorTrait};
use crate::infrastructure::http::waf_engine::WafInspector;
use crate::infrastructure::observability::log_scrape_error;
#[allow(unused_imports)]
use async_compression::tokio::bufread::GzipDecoder;
use futures::future::BoxFuture;
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tracing::Instrument;
use url::Url;

// SitemapError, SitemapUrl and the sitemap `Result` alias moved to
// `domain::crawler_port::sitemap` in the sitemap port slice (ADR-0012-B,
// follow-up of #1082). Re-exported here so the `infrastructure::crawler`
// root paths keep resolving unchanged — the repo's blessed move+shim
// migration (ADR-0012 §6 tradeoff 2).
pub use crate::domain::crawler_port::sitemap::{Result, SitemapError, SitemapUrl};

mod fetch;
mod parse;
mod url_resolution;
mod xml_core;
pub use url_resolution::resolve_url;
pub use xml_core::parse_sitemap;

/// Bounded buffered sitemap parser.
///
/// The parser buffers, it does not stream (#1822): the response body is
/// accumulated up to `SitemapConfig::max_response_size`, decompressed up to
/// `SitemapConfig::max_decompressed_size`, and the aggregate URL set of a
/// sitemap index is held under `SitemapConfig::memory_limit_mb`. Only one
/// body buffer (raw or decompressed) is retained while parsing; during
/// decompression itself the compressed input and the decompressed output
/// coexist transiently — inherent to the bounded two-layer re-sniff design
/// (#757) — and both sides are capped.
pub struct SitemapParser {
    config: SitemapConfig,
    compression_handler: CompressionHandler,
    url_validator: UrlValidator,
    retry_policy: RetryPolicy,
    memory_manager: MemoryManager,
    batch_processor: BatchProcessor,
    /// TLS/HTTP2 fingerprint emulation preset applied to the sitemap fetch client.
    ///
    /// Threaded from the caller (ultimately the CLI `--h2-profile` value).
    /// Defaults to [`wreq_util::Profile::Chrome145`], preserving the historical
    /// sitemap fingerprint (#323).
    tls_emulation: wreq_util::Profile,
    /// Shared HTTP client used to fetch sitemap XML.
    ///
    /// Built once in the constructor from `tls_emulation` via the shared
    /// `create_http_client_with_config` factory (#299) and cloned cheaply
    /// (it is `Arc`-backed) into each retry attempt — no per-retry rebuild (#323).
    http_client: wreq::Client,
}

impl SitemapParser {
    /// Create new parser with default config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn new() -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(SitemapConfig::default(), wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config
    ///
    /// Uses the [`wreq_util::Profile::Chrome145`] TLS fingerprint (historical
    /// default). For a caller-supplied profile, use
    /// [`SitemapParser::with_config_and_profile`].
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config(config: SitemapConfig) -> std::result::Result<Self, CrawlError> {
        Self::with_config_and_profile(config, wreq_util::Profile::Chrome145)
    }

    /// Create new parser with custom config and an explicit TLS/H2 profile.
    ///
    /// The sitemap fetch client is built once here (via the shared
    /// `create_http_client_with_config` factory) from `tls_emulation` and reused
    /// across every fetch and retry, honoring the caller's `--h2-profile`
    /// selection instead of a hardcoded preset (#323).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the URL validator's HTTP client or the
    /// sitemap fetch client fails to build.
    pub fn with_config_and_profile(
        config: SitemapConfig,
        tls_emulation: wreq_util::Profile,
    ) -> std::result::Result<Self, CrawlError> {
        let http_client = Self::build_client(tls_emulation, config.timeout)?;
        let max_decompressed_size = config.max_decompressed_size;
        let memory_limit_mb = config.memory_limit_mb;
        Ok(Self {
            config,
            compression_handler: CompressionHandler::with_max_size(max_decompressed_size),
            url_validator: UrlValidator::with_profile(tls_emulation)?,
            retry_policy: RetryPolicy::new(),
            // #1822: honor the configured budget instead of a second hardcoded
            // 500 MB (`MemoryManager::new()`). The builder's default is still
            // 500 MB, so callers that never set `memory_limit_mb` see no change.
            memory_manager: MemoryManager::with_memory_limit(memory_limit_mb),
            batch_processor: BatchProcessor::new(),
            tls_emulation,
            http_client,
        })
    }

    /// Build the sitemap fetch HTTP client for a given TLS/H2 profile.
    ///
    /// `timeout` is the #2490 inheritance: `None` keeps the parser's private
    /// default (10s request + 10s connect, the historical behavior of the
    /// previous hardcoded client), `Some(d)` is the operator's `--timeout-secs`
    /// inherited through `SitemapConfig` — each request is then bounded at `d`
    /// with the #281 connect policy (connect capped at 10s). The remaining
    /// settings (Chrome Client Hints, pool tuning, compression) come from the
    /// shared factory defaults (#299).
    ///
    /// # Errors
    ///
    /// Returns `CrawlError::Internal` if the underlying client fails to build.
    fn build_client(
        tls_emulation: wreq_util::Profile,
        timeout: Option<std::time::Duration>,
    ) -> std::result::Result<wreq::Client, CrawlError> {
        let request_timeout = timeout.unwrap_or(std::time::Duration::from_secs(10));
        let request_secs = request_timeout.as_secs();
        let http_config = crate::domain::http_config::HttpClientConfig {
            tls_emulation,
            timeout_secs: request_secs,
            // #281 policy: connect timeout capped at 10s.
            connect_timeout_secs: request_secs.min(10),
            ..Default::default()
        };
        crate::infrastructure::http::create_http_client_with_config(&http_config)
            // LCOV_EXCL_LINE defensive: wreq-client-build — client construction fails only on invalid TLS profile, an invariant
            .map_err(|e| CrawlError::Internal(format!("failed to build sitemap client: {e}")))
    }

    /// Check if gzip is enabled in config
    #[must_use]
    pub fn has_gzip(&self) -> bool {
        self.config.gzip_enabled
    }

    /// Get current max depth
    #[must_use]
    pub fn max_depth(&self) -> u8 {
        self.config.max_depth
    }

    /// Get the TLS/HTTP2 fingerprint emulation preset used for sitemap fetches.
    #[must_use]
    pub fn tls_emulation(&self) -> wreq_util::Profile {
        self.tls_emulation
    }
}

/// Domain-port implementation (ADR-0012-B sitemap port). The `parse_from_url`
/// logic moved here from the inherent method; the inherent `parse_from_url`
/// is now a thin wrapper so infrastructure internals and integration-test
/// call sites keep compiling unchanged.
///
/// This is no longer the correlation root of a sitemap parse; instead, it
/// derives its span correlation from the caller's run-root [`CorrelationId`]
/// via `correlation.child()` so the parse joins the caller's trace (#1386).
impl SitemapParserPort for SitemapParser {
    fn parse_from_url<'a>(
        &'a self,
        sitemap_url: &'a str,
        correlation: &'a CorrelationId,
    ) -> BoxFuture<'a, Result<Vec<SitemapUrl>>> {
        // Derive a child correlation for this parse so it joins the caller's trace
        let child_correlation = correlation.child();
        // The span must own its recorded values, and the future must own the
        // correlation it propagates: clone before the `async move` captures.
        let span = tracing::info_span!(
            "sitemap.parse",
            url = %sitemap_url,
            correlation_id = %child_correlation,
            trace_id = %child_correlation.trace_id()
        );
        Box::pin(
            async move {
                let visited = Arc::new(Mutex::new(HashSet::new()));
                self.parse_with_depth(
                    sitemap_url,
                    self.config.max_depth,
                    &visited,
                    &child_correlation,
                )
                .await
            }
            .instrument(span),
        )
    }
}

#[cfg(test)]
mod tests_compression;
#[cfg(test)]
mod tests_config;
#[cfg(test)]
mod tests_fetch;
#[cfg(test)]
mod tests_resolve_url;
#[cfg(test)]
mod tests_support;
#[cfg(test)]
use tests_support as test_support;
#[cfg(test)]
mod tests_waf;
