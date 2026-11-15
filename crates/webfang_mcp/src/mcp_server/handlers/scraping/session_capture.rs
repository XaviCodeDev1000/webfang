use super::*;

impl McpHandler {
    /// Convert this crawl run's captured pages into the session-owned
    /// result set the export tools consume (#1290, P6-2/F-16).
    ///
    /// Pages go through the SINGLE shared conversion path —
    /// `extract_page_content`, the same helper the CLI batch/export phases
    /// use — so record equivalence holds by construction: same DTO, same
    /// `WebfangMetadata` downstream (checksum, timestamp, `word_count`,
    /// `metadata_version`). Per-page extraction failures are logged through
    /// the shared `log_scrape_error` path and skipped: one bad page never
    /// drops the run's good records, exactly as on the CLI batch path.
    ///
    /// The buffer is REPLACED wholesale per run: an export after a second
    /// crawl mirrors the second crawl, and an empty extraction result
    /// honestly reads as "no hay resultados disponibles para exportar".
    /// Lock discipline follows `metrics` (REQ-07): the synchronous lock is
    /// taken only to swap the vector, never across an `.await`.
    ///
    /// # Budget (#1611, F7)
    ///
    /// A run whose records do not fit
    /// [`McpState::session_result_budget_bytes`](crate::mcp_server::McpState::session_result_budget_bytes) is refused as a whole
    /// ([`within_session_budget`]) and the buffer is EMPTIED rather than left
    /// holding the previous run: a stale buffer would make the next export
    /// serve a different crawl than the tool chain the client just ran, which
    /// is the same "success whose body reads like a failure" shape the error
    /// contract forbids. The caller turns the error into `isError: true` (see
    /// `session_budget_message`), so the client learns that the run produced
    /// nothing exportable instead of discovering it at export time.
    ///
    /// # Errors
    ///
    /// Returns [`SessionResultOverflow`] when the run exceeds the budget.
    /// Page-extraction failures are NOT errors here: they are logged and
    /// skipped, exactly as on the CLI batch path.
    pub(super) async fn store_session_results(
        &self,
        sink: &webfang_core::application::crawler::content_sink::InMemoryContentSink,
    ) -> Result<(), SessionResultOverflow> {
        let (results, failures) = self.convert_captured_pages(sink).await;
        let captured = results.len();
        let budget = self.state.session_result_budget_bytes.get();
        let overflow = match within_session_budget(results, budget) {
            Ok(admitted) => {
                self.swap_session_results(admitted, captured, failures, false);
                return Ok(());
            },
            Err(overflow) => {
                tracing::warn!(
                    operation = SESSION_RESULT_OPERATION,
                    budget_bytes = overflow.budget_bytes,
                    total_bytes = overflow.total_bytes,
                    captured,
                    failures,
                    kept = overflow.kept,
                    dropped = overflow.dropped,
                    "mcp_session_results_over_budget — refusing the whole run"
                );
                overflow
            },
        };
        // The swap happens either way: on refusal the buffer is EMPTIED, so a
        // later export cannot serve the PREVIOUS run's records as if they were
        // this one's (see the # Errors note above).
        self.swap_session_results(Vec::new(), captured, failures, true);
        Err(overflow)
    }

    /// Convert every page the crawl captured into its exportable record,
    /// returning the records and how many pages failed to convert.
    ///
    /// Split out of [`Self::store_session_results`] only to keep both halves
    /// readable: the conversion loop and the budget admission are independent
    /// concerns, and the loop's per-page failure handling is what pushes the
    /// combined function past the repo's cognitive-complexity ratchet.
    ///
    /// A failed page is logged through the shared `log_scrape_error` path and
    /// skipped — one bad page never drops the run's good records, exactly as
    /// on the CLI batch path.
    pub(super) async fn convert_captured_pages(
        &self,
        sink: &webfang_core::application::crawler::content_sink::InMemoryContentSink,
    ) -> (Vec<webfang_core::domain::ScrapedContent>, usize) {
        use webfang_core::application::crawler::content_sink::extract_page_content;
        use webfang_core::domain::CorrelationId;
        use webfang_core::infrastructure::observability::log_scrape_error;

        // Same shape as the CLI batch phase: default extraction config
        // (selector/ignore-waf defaults), output dir carried from the
        // container, one run-root identity with per-page children.
        let config = webfang_core::domain::config::ScraperConfig {
            output_dir: self.state.container.scraper_config.output_dir.clone(),
            ..Default::default()
        };
        let root = CorrelationId::new();
        let pages = sink.take_pages();
        let mut results = Vec::with_capacity(pages.len());
        let mut failures = 0usize;
        for page in &pages {
            let child = root.child();
            match extract_page_content(page, &config, &child).await {
                Ok(content) => results.push(content),
                Err((url, err)) => {
                    failures += 1;
                    log_scrape_error(
                        &err,
                        &url,
                        "mcp_session_capture",
                        Some(&child),
                        "crawl result extraction failed",
                    );
                },
            }
        }
        (results, failures)
    }

    /// Replace this session's result buffer, under its lock, with whatever the
    /// run was allowed to keep.
    ///
    /// Lock discipline (REQ-07): the guard is taken, swapped and dropped
    /// entirely inside this synchronous function — no `.await` runs while it
    /// is held. `refused` selects the event shape, because "cleared" and
    /// "captured" are different facts and a client reading the trace must not
    /// have to infer which happened from a missing field.
    pub(super) fn swap_session_results(
        &self,
        admitted: Vec<webfang_core::domain::ScrapedContent>,
        captured: usize,
        failures: usize,
        refused: bool,
    ) {
        match self.state.session_results.lock() {
            Ok(mut guard) => {
                let replaced = guard.len();
                *guard = admitted;
                if refused {
                    tracing::warn!(replaced, "mcp_session_results_cleared_on_over_budget");
                } else {
                    tracing::info!(captured, replaced, failures, "mcp_session_results_captured");
                }
            },
            Err(poisoned) => {
                // Poisoned elsewhere only by a panic mid-swap: recover the
                // guard and keep the run's records rather than losing them.
                let mut guard = poisoned.into_inner();
                let replaced = guard.len();
                *guard = admitted;
                tracing::warn!(
                    captured,
                    failures,
                    replaced,
                    "mcp_session_results_lock_recovered"
                );
            },
        }
    }
}
