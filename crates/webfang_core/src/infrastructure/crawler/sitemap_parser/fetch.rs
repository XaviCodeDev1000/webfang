use super::*;

impl SitemapParser {
    /// Parse sitemap from URL (bounded buffered parse).
    ///
    /// The response body is buffered up to `SitemapConfig::max_response_size`
    /// and decompressed up to `SitemapConfig::max_decompressed_size`; for a
    /// sitemap index, children are parsed with bounded concurrency and the
    /// aggregate URL set is held under `SitemapConfig::memory_limit_mb`
    /// ([`SitemapError::MemoryLimitExceeded`] beyond it, #1822). The combined
    /// result is deduplicated and sorted.
    ///
    /// # Arguments
    ///
    /// * `url` - Sitemap URL (supports .xml and .xml.gz)
    /// * `correlation` - The caller's run-root [`CorrelationId`]; the impl derives
    ///   its span correlation as `correlation.child()` so the parse joins the
    ///   caller's trace instead of minting a second root (#1386).
    ///
    /// # Returns
    ///
    /// Vector of valid URLs found in sitemap
    ///
    /// # Errors
    ///
    /// Returns `SitemapError` if parsing fails or no URLs found
    pub async fn parse_from_url(
        &self,
        url: &str,
        correlation: &CorrelationId,
    ) -> Result<Vec<SitemapUrl>> {
        SitemapParserPort::parse_from_url(self, url, correlation).await
    }

    /// Validate a sitemap HTTP response: status MUST be checked before
    /// content-type so a 404/5xx yields "not found" rather than the misleading
    /// "unexpected content-type" (issue #590, bug #9). Returns the content-type
    /// string on success for the caller's XML streaming path.
    ///
    /// Rejections are reported through [`log_scrape_error`] with `url`,
    /// `stage = "sitemap.fetch"` and the caller's correlation (#1318).
    fn validate_response(
        status: wreq::StatusCode,
        content_type: &str,
        url: &str,
        correlation: &CorrelationId,
    ) -> Result<()> {
        if !status.is_success() {
            let err = SitemapError::HttpError {
                status: status.as_u16(),
                message: format!("server returned {status}"),
            };
            log_scrape_error(
                &err,
                url,
                "sitemap.fetch",
                Some(correlation),
                "Sitemap URL returned non-2xx status",
            );
            return Err(err);
        }
        let is_xml = content_type.is_empty()
            || content_type.contains("application/xml")
            || content_type.contains("text/xml")
            || content_type.contains("application/xhtml+xml")
            || url.ends_with(".xml")
            || url.ends_with(".xml.gz");
        if !is_xml {
            let err = SitemapError::InvalidContentType(content_type.to_string());
            log_scrape_error(
                &err,
                url,
                "sitemap.fetch",
                Some(correlation),
                "Sitemap URL returned non-XML content type",
            );
            return Err(err);
        }
        Ok(())
    }

    /// Internal recursive parser with depth tracking and loop detection
    ///
    /// Carries the run's [`CorrelationId`] so every failure in the chain is
    /// logged through [`log_scrape_error`] with `stage` and `trace_id`
    /// (#1318).
    ///
    /// Body handling is bounded, not streaming (#1822): the response is
    /// accumulated up to `max_response_size`, then decompressed by
    /// [`Self::parse_decompressed_body`]'s caller — decompression consumes
    /// the raw buffer, returning the same allocation when no compression was
    /// detected — so only one body buffer stays live during the parse.
    // The #1318 migration replaced four inline `warn!` one-liners with
    // `log_scrape_error` calls that carry the correlation at each failure
    // stage; the fetch→validate→stream sequence stays one cohesive span body
    // above the line ratchet — same treatment as `parse_xml_sitemap` and
    // `scrape_single_url_inner`. (#1822 extracted the post-decompression
    // tail into `parse_decompressed_body`.)
    #[allow(clippy::too_many_lines)]
    #[tracing::instrument(
        level = "debug",
        name = "sitemap.parse_url",
        skip(self, visited, correlation),
        fields(
            url = %url,
            depth = depth,
            correlation_id = %correlation,
            trace_id = %correlation.trace_id()
        )
    )]
    pub(super) async fn parse_with_depth(
        &self,
        url: &str,
        depth: u8,
        visited: &Arc<Mutex<HashSet<Url>>>,
        correlation: &CorrelationId,
    ) -> Result<Vec<SitemapUrl>> {
        // Base case: max depth reached
        if depth == 0 {
            return Err(SitemapError::MaxDepthExceeded);
        }

        let base_url = Url::parse(url)?;

        // Loop detection: skip already-visited URLs
        {
            let visited_lock = visited.lock().map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("failed to acquire visited lock: {e}"),
            })?;
            if visited_lock.contains(&base_url) {
                let err = SitemapError::InvalidStructure;
                log_scrape_error(
                    &err,
                    url,
                    "sitemap.fetch",
                    Some(correlation),
                    "Skipping already-visited sitemap (loop detected)",
                );
                return Err(err);
            }
        }

        // #1382: entry guard before ANY socket — a sitemap URL (the initial
        // target or an index `<sitemap>` child, which is server-controlled
        // text) whose host is a forbidden IP literal is cut here, layer 1 of
        // the AGENTS.md guard-chain order. Pre-socket by construction: the
        // retry-wrapped fetch below never runs for a refused target.
        if let Ok(parsed) = Url::parse(url) {
            if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(&parsed)
            {
                tracing::warn!(
                    url = url,
                    "SSRF literal-IP sitemap target rejected at parser entry (no socket opened)"
                );
                return Err(SitemapError::SsrfLiteralRejected(rejection.to_string()));
            }
        }

        // [3.6] RetryPolicy: wrap HTTP request with retry logic.
        // The client is built once in the constructor (honoring tls_emulation)
        // and cloned cheaply (Arc-backed) into each attempt — no per-retry rebuild (#323).
        let response = self
            .retry_policy
            .execute_with_retry(|| {
                let url = url.to_string();
                let client = self.http_client.clone();
                async move { client.get(&url).send().await }
            })
            .await
            .map_err(|e| SitemapError::HttpError {
                status: 0, // Unknown status for network/connection errors
                message: e.to_string(),
            })?;

        // Validate response: status checked before content-type (issue #590, bug #9).
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        Self::validate_response(status, content_type.as_str(), url, correlation)?;

        // Mark URL as visited after successful fetch
        {
            let mut visited_lock = visited.lock().map_err(|e| SitemapError::HttpError {
                status: 0,
                message: format!("failed to acquire visited lock: {e}"),
            })?;
            visited_lock.insert(base_url.clone());
        }

        // Stream response with size limit
        use futures::StreamExt;
        let mut stream = response.bytes_stream();
        let mut raw_bytes = Vec::with_capacity(8192);
        let mut total_bytes = 0usize;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| SitemapError::HttpError {
                status: 0, // Stream errors are connection errors
                message: e.to_string(),
            })?;
            total_bytes += chunk.len();
            if total_bytes > self.config.max_response_size {
                let err = SitemapError::ResponseTooLarge(self.config.max_response_size);
                log_scrape_error(
                    &err,
                    url,
                    "sitemap.fetch",
                    Some(correlation),
                    &format!("Sitemap response too large: {total_bytes} bytes streamed"),
                );
                return Err(err);
            }
            raw_bytes.extend_from_slice(&chunk);
        }

        // [3.4] CompressionHandler integration: detect and decompress content.
        // Consumes `raw_bytes`: when no compression is detected the same
        // allocation comes back — the single live body buffer — otherwise the
        // decompressed output replaces it and the compressed input drops.
        // Both sides are capped (`max_response_size` /
        // `max_decompressed_size`); during the decompression itself the
        // compressed input and the decompressed output coexist transiently,
        // which is inherent to the bounded two-layer re-sniff design (#757).
        let body = self
            .compression_handler
            .detect_and_decompress(raw_bytes, url)
            .await
            .map_err(|e| SitemapError::DecompressionError(e.to_string()))?;

        self.parse_decompressed_body(
            &body,
            url,
            &base_url,
            status.as_u16(),
            &content_type,
            depth,
            visited,
            correlation,
        )
        .await
    }
}
