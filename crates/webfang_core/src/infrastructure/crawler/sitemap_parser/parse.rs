use super::xml_core::{dedup_and_sort_sitemap_urls, parse_sitemap_core};
use super::*;

impl SitemapParser {
    /// Shared tail of [`Self::parse_with_depth`]: WAF inspection, XML parse,
    /// the per-parse memory gate, then index recursion or the crawl-budget
    /// pass.
    ///
    /// Splitting the tail out keeps `parse_with_depth` under the line ratchet
    /// and carries the single-buffer contract (#1822): the body handed here
    /// is the only live body buffer for the whole parse.
    ///
    /// Observability: runs inside the caller's `sitemap.parse_url` span; every
    /// failure path logs through [`log_scrape_error`] with the caller's
    /// correlation (#1318).
    // The parameters mirror the caller's span state one-to-one; bundling them
    // into a struct would add a type for a single private call site.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn parse_decompressed_body(
        &self,
        body: &[u8],
        url: &str,
        base_url: &Url,
        status: u16,
        content_type: &str,
        depth: u8,
        visited: &Arc<Mutex<HashSet<Url>>>,
        correlation: &CorrelationId,
    ) -> Result<Vec<SitemapUrl>> {
        // Issue #879 (Option A): inspect the body BEFORE XML parsing so a WAF
        // challenge served where sitemap content should be surfaces as the
        // typed error instead of generic XML-garbage failures.
        if let Some(err) = Self::waf_challenge_error(body, url, status, content_type, correlation) {
            return Err(err);
        }

        // Parse using unified decompression handle
        let (urls, is_index) = if body.is_empty() {
            return Err(SitemapError::NoUrlsFound);
        } else {
            self.parse_xml_sitemap(body, base_url).await?
        };

        // [3.7] MemoryManager: handle disk swapping for large result sets
        // (both branches need it, so hoist it above the index/urlset split).
        self.memory_manager
            .handle_disk_swapping(&urls)
            .map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("memory management failed: {e}"),
            })?;

        // Check if sitemap index (recursive) - Bug 7: use root element detection
        if is_index {
            tracing::debug!(
                "Detected sitemap index (root element), recursing (depth: {})",
                depth
            );

            self.parse_sitemap_index(&urls, depth - 1, visited, correlation)
                .await
        } else {
            // [3.8] BatchProcessor: apply crawl budget optimization
            let optimized_urls = self.batch_processor.apply_crawl_budget(urls, &self.config);

            Ok(optimized_urls)
        }
    }

    /// Parse gzip-compressed sitemap
    #[allow(dead_code)]
    async fn parse_gzip_sitemap(&self, bytes: &[u8], base_url: &Url) -> Result<Vec<SitemapUrl>> {
        use tokio::io::{AsyncReadExt, BufReader};
        let reader = BufReader::new(bytes);
        let mut decoder = GzipDecoder::new(reader);

        let mut limited =
            AsyncReadExt::take(&mut decoder, self.config.max_decompressed_size as u64);
        let mut decompressed = Vec::new();
        AsyncReadExt::read_to_end(&mut limited, &mut decompressed).await?;

        if decompressed.len() >= self.config.max_decompressed_size {
            tracing::warn!(
                bytes = decompressed.len(),
                limit = self.config.max_decompressed_size,
                "gzip decompression hit size limit — possible decompression bomb"
            );
            return Err(SitemapError::DecompressedTooLarge(
                self.config.max_decompressed_size,
            ));
        }

        let (urls, _is_index) = self.parse_xml_sitemap(&decompressed, base_url).await?;
        Ok(urls)
    }

    /// Parse XML sitemap (bounded buffered parse over the in-memory body)
    /// with metadata extraction
    ///
    /// Extracts `<loc>`, `<lastmod>`, `<priority>`, and `<changefreq>` from
    /// `<url>` entries per sitemaps.org spec. Also handles `<sitemap>` entries
    /// in sitemap index files.
    #[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
    async fn parse_xml_sitemap(
        &self,
        bytes: &[u8],
        base_url: &Url,
    ) -> Result<(Vec<SitemapUrl>, bool)> {
        let mut reader = Reader::from_reader(bytes);
        let (mut urls, is_index) =
            parse_sitemap_core(&mut reader, base_url, true, Some(&self.url_validator))?;

        if urls.is_empty() {
            return Err(SitemapError::NoUrlsFound);
        }
        dedup_and_sort_sitemap_urls(&mut urls);
        Ok((urls, is_index))
    }

    /// Inspect a fetched sitemap-chain body for WAF/CAPTCHA challenges
    /// (issue #879).
    ///
    /// Reuses [`WafInspector`] with full HTTP context (status/content-type are
    /// known here), so verdict semantics match the HTTP client path exactly:
    /// Challenge-tier markers block at any status, Fingerprint-tier evidence
    /// blocks only when correlated with a WAF status code (REQ-WAF-05/09).
    /// Control headers are not re-collected here because `validate_response`
    /// guarantees a 2xx status, under which Fingerprint evidence never blocks.
    /// The block is reported through [`log_scrape_error`] with stage and the
    /// caller's correlation (#1318).
    fn waf_challenge_error(
        body: &[u8],
        url: &str,
        status: u16,
        content_type: &str,
        correlation: &CorrelationId,
    ) -> Option<SitemapError> {
        let ctx = InspectionContext {
            status: Some(status),
            content_type: (!content_type.is_empty()).then(|| content_type.to_string()),
            headers: std::collections::HashMap::new(),
            ignore_waf: false,
        };
        let text = String::from_utf8_lossy(body);
        let verdict = WafInspector::inspect(&text, &ctx);
        if !verdict.is_blocked {
            return None;
        }
        let err = SitemapError::WafChallenge {
            url: url.to_string(),
            provider: verdict.evidence_chain(),
        };
        log_scrape_error(
            &err,
            url,
            "sitemap.fetch",
            Some(correlation),
            "WAF/CAPTCHA challenge detected in sitemap body; aborting",
        );
        Some(err)
    }

    /// Parse sitemap index recursively with error propagation
    ///
    /// Each index child fetch/parse gets its own [`CorrelationId`] child so a
    /// failing child is reconstructable from the trace JSONL by `trace_id`
    /// while still sharing the run root's — and failures are surfaced through
    /// [`log_scrape_error`] with `stage = "sitemap.index_child"` (#1318).
    async fn parse_sitemap_index(
        &self,
        sitemap_urls: &[SitemapUrl],
        depth: u8,
        visited: &Arc<Mutex<HashSet<Url>>>,
        correlation: &CorrelationId,
    ) -> Result<Vec<SitemapUrl>> {
        use futures::stream::{self, StreamExt};

        let mut all_urls = Vec::new();
        let mut failures = Vec::new();

        // #1822: process children incrementally instead of collecting every
        // child's URL list up front. Results stream in submission order over
        // the SAME bounded-concurrency stream, so the aggregate budget check
        // below runs after every successful child and stops scheduling new
        // fetches once the combined URL set exceeds `memory_limit_mb`.
        let mut results = stream::iter(sitemap_urls.iter().cloned())
            .map(|sitemap_url| {
                let visited = visited.clone();
                let child_correlation = correlation.child();
                async move {
                    let url = sitemap_url.url.clone();
                    let result = self
                        .parse_with_depth(url.as_str(), depth, &visited, &child_correlation)
                        .await;
                    (url, result, child_correlation)
                }
            })
            .buffered(self.config.concurrency);

        let mut children_completed = 0usize;
        while let Some((url, result, child_correlation)) = results.next().await {
            children_completed += 1;
            match result {
                Ok(urls) => {
                    all_urls.extend(urls);
                    self.enforce_aggregate_budget(
                        &all_urls,
                        url.as_str(),
                        children_completed,
                        correlation,
                    )?;
                },
                Err(e) => {
                    log_scrape_error(
                        &e,
                        url.as_str(),
                        "sitemap.index_child",
                        Some(&child_correlation),
                        "Failed to parse sitemap index child",
                    );
                    failures.push((url, e));
                },
            }
        }

        // Issue #879 (Option A): a challenge on ANY child hard-aborts the
        // whole index (HybridRouter parity) — the host is serving challenges,
        // not sitemaps — instead of being swallowed into AllChildrenFailed.
        for (_, e) in &failures {
            if let SitemapError::WafChallenge { url, provider } = e {
                tracing::warn!(
                    url = %url,
                    provider = %provider,
                    "WAF/CAPTCHA challenge detected in index child; aborting index"
                );
                return Err(SitemapError::WafChallenge {
                    url: url.clone(),
                    provider: provider.clone(),
                });
            }
            // #1382: a forbidden-literal child was cut by the entry guard —
            // a security-policy refusal is not a fetch failure to aggregate
            // into AllChildrenFailed; it hard-aborts the index with the
            // typed rejection so the Spanish SSRF copy surfaces (same
            // tier-priority as a WAF challenge on any child).
            if let SitemapError::SsrfLiteralRejected(msg) = e {
                return Err(SitemapError::SsrfLiteralRejected(msg.clone()));
            }
        }

        if all_urls.is_empty() {
            if !failures.is_empty() {
                // Split children that were valid but EMPTY (NoUrlsFound) from
                // children that genuinely failed to fetch/parse. An index whose
                // children are all empty is "no URLs discovered" (exit 2 via
                // NoUrlsFound→SitemapEmpty), NOT an infrastructure failure; only
                // real fetch/parse failures yield AllChildrenFailed (exit 69 via
                // Parse→Internal). Previously both collapsed into
                // AllChildrenFailed and the CLI string-matched the message to
                // tell them apart (stabilization-sitemap-regression).
                let (empty_children, real_failures): (Vec<_>, Vec<_>) = failures
                    .into_iter()
                    .partition(|(_, e)| matches!(e, SitemapError::NoUrlsFound));
                if real_failures.is_empty() {
                    tracing::debug!(
                        "all {} child sitemaps were empty — no URLs discovered",
                        empty_children.len()
                    );
                    return Err(SitemapError::NoUrlsFound);
                }
                let failure_msgs: Vec<String> = real_failures
                    .iter()
                    .map(|(url, e)| format!("{url}: {e}"))
                    .collect();
                return Err(SitemapError::AllChildrenFailed(
                    real_failures.len(),
                    failure_msgs.join("; "),
                ));
            }
            Err(SitemapError::NoUrlsFound)
        } else {
            dedup_and_sort_sitemap_urls(&mut all_urls);
            Ok(all_urls)
        }
    }

    /// Enforce the aggregate memory budget across all index children (#1822).
    ///
    /// [`MemoryManager::ensure_within_budget`] charges ~2 KB per URL against
    /// `SitemapConfig::memory_limit_mb`; the check runs after every successful
    /// child so the combined URL set of the index can never grow past the
    /// budget. On exceed, the failure is reported through [`log_scrape_error`]
    /// with `stage = "sitemap.index"` and a structured warn records the run's
    /// progress (children completed, URLs accumulated, configured limit) so
    /// the abort is reconstructable from the trace JSONL.
    ///
    /// # Errors
    ///
    /// Returns [`SitemapError::MemoryLimitExceeded`] when the aggregate URL
    /// set exceeds the configured budget, or the same `HttpError` mapping the
    /// per-parse memory check uses should the manager report anything else.
    fn enforce_aggregate_budget(
        &self,
        all_urls: &[SitemapUrl],
        child_url: &str,
        children_completed: usize,
        correlation: &CorrelationId,
    ) -> Result<()> {
        let Err(memory_err) = self.memory_manager.ensure_within_budget(all_urls) else {
            return Ok(());
        };
        let err = match memory_err {
            MemoryError::MemoryLimitExceeded(estimated_mb) => {
                SitemapError::MemoryLimitExceeded(estimated_mb)
            },
            // Defensive: `ensure_within_budget` never touches disk; map any
            // unexpected variant through the same bucket the per-parse memory
            // check uses so the stratification holds.
            // LCOV_EXCL_START defensive: ensure_within_budget never touches disk, unreachable from the aggregate budget path
            MemoryError::DiskSwapFailed(msg) => SitemapError::HttpError {
                status: 0,
                message: format!("memory management failed: {msg}"),
            },
            // LCOV_EXCL_STOP
        };
        log_scrape_error(
            &err,
            child_url,
            "sitemap.index",
            Some(correlation),
            "aggregate sitemap URL set exceeded the configured memory limit",
        );
        tracing::warn!(
            children_completed = children_completed,
            urls_accumulated = all_urls.len(),
            limit_mb = self.memory_manager.memory_limit_mb(),
            "sitemap index aggregate exceeded the memory budget; aborting remaining children"
        );
        Err(err)
    }
}

#[cfg(all(test, not(miri)))]
#[path = "tests_xml.rs"]
mod tests_xml;
