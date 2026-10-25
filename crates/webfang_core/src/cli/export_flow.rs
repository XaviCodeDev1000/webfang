//! Export flow — handles result export (standard and AI-cleaned) and file saving.

use std::path::{Path, PathBuf};

use futures::future::FutureExt;
use futures::stream::StreamExt;
use tracing::warn;

use crate::cli::error::CliExit;
use crate::cli::scrape_flow::{record_store_bridge, warn_unreadable_state};
use crate::domain::persistence::StateStorePort;
use crate::domain::ScrapedContent;
use crate::infrastructure::output::file_saver::ObsidianOptions;
use crate::{
    application::export_factory, infrastructure::output::file_saver::save_results, ExportFormat,
    OutputFormat,
};

use tracing::info;

use crate::domain::semantic_cleaner::SemanticCleaner;

use crate::domain::DocumentChunk;

use crate::error::SemanticError;

use crate::error::ErrorClass;

// ============================================================================
// Export Results (RAG pipeline)
// ============================================================================

/// Configuration for the export flow.
///
/// `results` is shared ownership (`Arc<[ScrapedContent]>`) rather than a
/// borrow: the export itself runs on the blocking pool (issue #1814, slice B
/// AC4), which requires `'static` data, while the caller (`orchestrator` /
/// `batch_flow`) keeps its own `Arc` handle for `report_phase` after the
/// export. Sharing the slice costs one refcount increment — never a clone of
/// page content.
#[allow(dead_code)]
pub struct ExportConfig<'a> {
    pub(crate) results: std::sync::Arc<[ScrapedContent]>,
    pub(crate) output_dir: PathBuf,
    pub(crate) format: OutputFormat,
    pub(crate) export_format: ExportFormat,
    pub(crate) clean_ai: bool,
    pub(crate) quick_save: bool,
    pub(crate) vault_path: Option<&'a PathBuf>,
    pub(crate) obsidian_options: ObsidianOptions,
    pub(crate) state_store: Option<&'a dyn StateStorePort>,
    pub(crate) resume: bool,
    /// Run's cooperative cancellation token (#2244 decision A), cloned from
    /// the `ShutdownGuard` at the `export_phase` call site.
    ///
    /// Owned rather than borrowed: the `ResumeContext` is built INSIDE
    /// `spawn_blocking` (#1814 slice B), which needs `'static` data — a
    /// `&CancellationToken` borrowed from the async stack cannot cross that
    /// boundary. `CancellationToken` is clone-by-design, so the token enters
    /// the closure as its own clone and `observing_cancel` borrows that
    /// closure-local clone.
    pub(crate) cancel: Option<tokio_util::sync::CancellationToken>,
    /// AI settings (only used when clean_ai is true and feature is enabled)
    pub(crate) ai_threshold: f32,
    pub(crate) ai_max_chars: usize,
    pub(crate) ai_offline: bool,
}

/// Run the export flow: AI-cleaned or standard export.
///
/// Returns the list of processed URLs on success.
pub async fn run_export(
    config: ExportConfig<'_>,
    ai_cleaner: Option<std::sync::Arc<dyn SemanticCleaner>>,
) -> Result<Vec<String>, CliExit> {
    if config.clean_ai {
        match ai_cleaner {
            Some(cleaner) => run_ai_export(config, cleaner).await,
            None => Err(CliExit::ConfigError(
                "Se solicitó limpieza semántica AI pero el limpiador no está disponible (no se propagó una falla de inicialización)"
                    .into(),
            )),
        }
    } else {
        run_standard_export(config).await
    }
}

/// Surface the shared Spanish unreadable-state notice on the export resume
/// path (#1587): the export path previously resumed silently over unreadable
/// state while the scrape path warned. Single call site helper so both export
/// flows (standard + AI) stay identical without duplicating the bridge check.
fn warn_unreadable_resume_state(record_store: &Option<crate::infrastructure::export::RecordStore>) {
    if let Some(store) = record_store.as_ref() {
        warn_unreadable_state(store);
    }
}

/// Build the [`export_factory::ResumeContext`] the blocking export runs with:
/// the store to resume into, the `--resume` gate, and the run's cancellation
/// token (#2244 decision A) — the token is what makes a cancelled export DRAIN
/// instead of running to completion.
///
/// Called INSIDE `spawn_blocking`, on data the closure already owns: the token
/// argument is always the closure-local clone, never a borrow of the async
/// stack (the `'static` trap documented on `ExportConfig::cancel`).
fn resume_context<'a>(
    record_store: Option<&'a crate::infrastructure::export::RecordStore>,
    resume: bool,
    cancel: Option<&'a tokio_util::sync::CancellationToken>,
) -> Option<export_factory::ResumeContext<'a>> {
    record_store.map(|store| {
        let ctx = export_factory::ResumeContext::new(store).with_resume(resume);
        match cancel {
            Some(token) => ctx.observing_cancel(token),
            None => ctx,
        }
    })
}

/// Standard export path (backward compatible).
///
/// Issue #1814 (slice B, AC4): `process_results` is sync filesystem I/O plus a
/// SHA-256 per item plus resume-gate state writes, so it runs on the blocking
/// pool. The owned `RecordStore` and the shared `Arc<[ScrapedContent]>` move
/// into the closure — the D3 resume-gate ordering is untouched, only where the
/// work executes.
async fn run_standard_export(config: ExportConfig<'_>) -> Result<Vec<String>, CliExit> {
    // Bridge the state-store port onto the v2 RecordStore seam (shared
    // helper — same directory + domain derivation as the scrape path).
    // Built BEFORE the spawn: `config.state_store` is a borrow of caller
    // state, while the owned bridge is what moves into the blocking task.
    let record_store = config.state_store.map(record_store_bridge);
    warn_unreadable_resume_state(&record_store);
    let resume = config.resume;
    let output_dir = config.output_dir;
    let export_format = config.export_format;
    let results = config.results;
    // Lifetime trap (#2244 decision A): the token moves into the blocking
    // closure as its OWN CLONE (owned), never as a borrow of the async
    // stack — `spawn_blocking` requires 'static data and the `ResumeContext`
    // is constructed inside the closure, borrowing the closure-local clone.
    let cancel = config.cancel;
    // Re-enter the current span inside the blocking task: `spawn_blocking`
    // does not propagate the tracing context, and spanless export events
    // would fall back to the run seed and split the run's trace_id
    // (trace_orphan_spawn_test caught it). The guard lives only inside the
    // synchronous task, never across an `.await` (async-rules compliant).
    let span = tracing::Span::current();
    let handle = tokio::task::spawn_blocking(move || {
        let _span_guard = span.enter();
        let ctx = resume_context(record_store.as_ref(), resume, cancel.as_ref());
        export_factory::process_results(&results, output_dir, export_format, "export", ctx.as_ref())
    });
    await_blocking_export(handle, "scrape", export_format).await
}

/// AI semantic cleaning export path.
async fn run_ai_export(
    config: ExportConfig<'_>,
    cleaner: std::sync::Arc<dyn SemanticCleaner>,
) -> Result<Vec<String>, CliExit> {
    info!(
        "Starting AI cleaning for {} pages concurrently...",
        config.results.len()
    );

    // Surface the Spanish unreadable-state notice BEFORE the long AI run so a
    // degraded resume is visible up front rather than after minutes of
    // cleaning (#1587). Read-only: `load()` never mutates the state file.
    let record_store = config.state_store.map(record_store_bridge);
    warn_unreadable_resume_state(&record_store);

    // #2270: the cleaning phase observes the run's token too, so a Ctrl-C
    // during it stops at the next page instead of running to the end (and
    // paying for embeddings the user already asked to stop paying for). The
    // token is borrowed here and moved into the blocking closure below.
    let cleaned_chunks = clean_all_pages(&config.results, &cleaner, config.cancel.as_ref()).await?;

    info!(
        "AI cleaning complete: {} chunks from {} pages",
        cleaned_chunks.len(),
        config.results.len()
    );

    // Issue #1814 (slice B, AC4): `process_results_with_chunks` is sync
    // filesystem I/O + SHA-256 per chunk + resume-gate state writes — dispatch
    // it to the blocking pool with the owned chunks and record store. The
    // D3 ordering (claim/decide/commit/final_persist) is unchanged.
    let resume = config.resume;
    let output_dir = config.output_dir;
    let export_format = config.export_format;
    // Same owned-clone lifetime trap as `run_standard_export` (#2244): the
    // token crosses `spawn_blocking` as its own clone, never as a borrow.
    let cancel = config.cancel;
    // Same span re-entry as `run_standard_export`: the trace context does not
    // cross `spawn_blocking` on its own.
    let span = tracing::Span::current();
    let handle = tokio::task::spawn_blocking(move || {
        let _span_guard = span.enter();
        let ctx = resume_context(record_store.as_ref(), resume, cancel.as_ref());
        export_factory::process_results_with_chunks(
            &cleaned_chunks,
            output_dir,
            export_format,
            "export",
            ctx.as_ref(),
        )
    });
    await_blocking_export(handle, "AI-cleaned", export_format).await
}

/// Await a blocking-pool export handle and translate its outcome onto the CLI
/// error surface.
///
/// Shared by the standard and AI export paths (issue #1814 slice B): the D3
/// sequence executes on the blocking pool; this wrapper only maps
/// `ExporterError` and `JoinError` onto `CliExit` with the Spanish
/// user-facing wording. Extracted so both `run_*_export` paths stay inside
/// the cognitive-complexity ratchet.
async fn await_blocking_export(
    handle: tokio::task::JoinHandle<Result<Vec<String>, crate::domain::exporter::ExporterError>>,
    stage: &'static str,
    format: ExportFormat,
) -> Result<Vec<String>, CliExit> {
    match handle.await {
        Ok(Ok(urls)) => Ok(urls),
        Ok(Err(e)) => {
            warn!(error = %e, format = ?format, "export of {stage} results failed");
            Err(CliExit::IoError(e.to_string()))
        },
        Err(join) => {
            warn!(stage, error = %join, format = ?format, "export task failed on the blocking pool");
            Err(CliExit::IoError(format!(
                "la exportación falló en el pool de bloqueo: {join}"
            )))
        },
    }
}

/// One page's cleaning outcome: its URL, the cleaner's result, and the source
/// page (needed for the raw-content fallback and for chunk enrichment).
type CleanedPage = (
    crate::domain::value_objects::ValidUrl,
    Result<Vec<DocumentChunk>, SemanticError>,
    ScrapedContent,
);

/// Why the cleaning loop stopped asking for pages.
enum NextPage {
    Ready(Box<CleanedPage>),
    /// Every page was folded.
    Exhausted,
    /// The token fired; the pages still in flight were dropped.
    Cancelled,
}

impl NextPage {
    fn ready(page: Option<CleanedPage>) -> Self {
        page.map_or(Self::Exhausted, |page| Self::Ready(Box::new(page)))
    }
}

/// Await the next page that finished cleaning, observing `cancel` first.
///
/// A result that is ALREADY available wins over the token: its embeddings were
/// paid for, and dropping it would discard exactly the cost the cancellation
/// check exists to protect (#2270).
async fn next_cleaned_page<F>(
    pending: &mut futures::stream::FuturesUnordered<F>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> NextPage
where
    F: std::future::Future<Output = CleanedPage>,
{
    if let Some(ready) = pending.next().now_or_never() {
        return NextPage::ready(ready);
    }
    match cancel {
        None => NextPage::ready(pending.next().await),
        Some(token) => tokio::select! {
            biased;
            () = token.cancelled() => NextPage::Cancelled,
            item = pending.next() => NextPage::ready(item),
        },
    }
}

/// Accumulator for the cleaning phase: the chunks kept so far plus the counts
/// the `#543` systemic-failure guard and the cancellation summary need.
///
/// Extracted from [`clean_all_pages`] to keep that function inside the
/// `too_many_lines` ratchet, the same way `await_blocking_export` was.
#[derive(Default)]
struct CleaningTally {
    chunks: Vec<DocumentChunk>,
    failed: usize,
    fallback: usize,
    first_error: Option<SemanticError>,
}

impl CleaningTally {
    fn with_capacity(pages: usize) -> Self {
        Self {
            chunks: Vec::with_capacity(pages * 2),
            ..Self::default()
        }
    }

    /// Fold ONE page's outcome: keep its chunks, or degrade to the raw content
    /// and record the failure. `Err` is reserved for an infrastructure-fatal
    /// error, which aborts the whole run (#581).
    fn fold(&mut self, page: CleanedPage) -> Result<(), CliExit> {
        let (url, chunks_result, result) = page;
        let chunks = match chunks_result {
            Ok(chunks) => chunks,
            Err(e) => return self.record_failure(url, e, &result),
        };
        if chunks.is_empty() {
            warn!(url = %url, "AI cleaner produced 0 chunks; using raw content fallback");
            self.chunks
                .push(DocumentChunk::from_scraped_content(&result));
            return Ok(());
        }
        // The cleaner produces chunks with empty url/title (it only sees raw
        // HTML). Enrich each chunk with identity from its source page so
        // validate() passes and export succeeds (#569).
        self.chunks.extend(
            chunks
                .into_iter()
                .map(|chunk| chunk.enrich_from_scraped_content(&result)),
        );
        Ok(())
    }

    /// Classify a page failure by operational severity (#581): an
    /// infrastructure fatal aborts the whole run instead of burning CPU on
    /// pages that will all fail; anything else degrades to raw content and is
    /// counted, so an all-fallback job still surfaces an error (#543).
    fn record_failure(
        &mut self,
        url: crate::domain::value_objects::ValidUrl,
        e: SemanticError,
        result: &ScrapedContent,
    ) -> Result<(), CliExit> {
        match e.classify() {
            ErrorClass::InternalFatal => Err(CliExit::ConfigError(format!(
                "Falló la infraestructura de IA (error fatal, no reintentable): {e}"
            ))),
            ErrorClass::DomainRecoverable => {
                warn!(
                    url = %url,
                    error = %e,
                    "chunk exceeds character limit; using raw content fallback"
                );
                self.chunks
                    .push(DocumentChunk::from_scraped_content(result));
                self.fallback += 1;
                if self.first_error.is_none() {
                    self.first_error = Some(e);
                }
                Ok(())
            },
            // SemanticError::classify() only returns InternalFatal or
            // DomainRecoverable, but handle the other classes exhaustively so
            // future variants don't silently fall through.
            ErrorClass::TransientRetriable
            | ErrorClass::TransientBackoff
            | ErrorClass::PermanentFatal => {
                self.failed += 1;
                crate::infrastructure::observability::log_scrape_error(
                    &e,
                    url.as_str(),
                    "ai_clean",
                    result.correlation_id.as_ref(),
                    "AI content cleanup failed; keeping raw content fallback",
                );
                if self.first_error.is_none() {
                    self.first_error = Some(e);
                }
                self.chunks
                    .push(DocumentChunk::from_scraped_content(result));
                Ok(())
            },
        }
    }

    /// EVERY page failed or fell back: the cause is systemic (not per-content),
    /// so the caller must propagate it instead of returning raw fallback chunks
    /// with a success exit code (#543 regression).
    fn systemic_failure(&self, pages: usize) -> Option<CliExit> {
        if pages == 0 || self.failed + self.fallback != pages {
            return None;
        }
        let detail = self.first_error.as_ref().map_or_else(
            || "sin detalle de error disponible".to_string(),
            ToString::to_string,
        );
        Some(CliExit::ConfigError(format!(
            "Falló la limpieza semántica AI en todas las páginas (error de modelo/configuración): {detail}"
        )))
    }
}

/// Run the cleaner over every scraped page concurrently and fold the results
/// into a single chunk list.
///
/// Per-page failures fall back to the raw content, but a TOTAL failure (every
/// page fails to clean) is treated as a model/config error and propagated as
/// `Err` instead of silently exiting 0 (#543).
///
/// Cancellation (#2270) is observed BETWEEN pages: the run's token is checked
/// before awaiting the next page, so a Ctrl-C during the cleaning phase stops
/// the run instead of letting it work — and pay for embeddings — until the
/// last page finishes. Pages that already produced chunks are folded and
/// returned (the caller exports them as the drain); pages still in flight are
/// dropped, and a cancelled run never escalates the `#543` total-failure
/// guard into a config error.
async fn clean_all_pages(
    results: &[ScrapedContent],
    cleaner: &std::sync::Arc<dyn SemanticCleaner>,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<Vec<DocumentChunk>, CliExit> {
    let mut pending: futures::stream::FuturesUnordered<_> = results
        .iter()
        .map(|result| {
            let html_content = result
                .html
                .clone()
                .unwrap_or_else(|| result.content.clone());
            let url = result.url.clone();
            let cleaner = std::sync::Arc::clone(cleaner);
            async move {
                let chunks_result = cleaner.clean(url.as_str(), &html_content).await;
                (url, chunks_result, result.clone())
            }
        })
        .collect();

    let mut tally = CleaningTally::with_capacity(results.len());
    let mut processed = 0usize;
    let mut cancelled = false;
    loop {
        match next_cleaned_page(&mut pending, cancel).await {
            NextPage::Ready(page) => {
                processed += 1;
                tally.fold(*page)?;
            },
            NextPage::Exhausted => break,
            NextPage::Cancelled => {
                cancelled = true;
                break;
            },
        }
    }

    if cancelled {
        info!(
            cleaned = processed,
            total = results.len(),
            "cancellation observed during AI cleaning; stopping before the next page"
        );
        // A cancelled run never escalates the systemic-failure guard into a
        // config error: it stopped early by request, and the CLI already maps
        // cancellation onto its own exit.
        return Ok(tally.chunks);
    }

    if let Some(exit) = tally.systemic_failure(results.len()) {
        return Err(exit);
    }

    Ok(tally.chunks)
}

// ============================================================================
// Save Individual Files (Markdown/Text/JSON)
// ============================================================================

/// Save individual output files with Obsidian support.
///
/// This is non-fatal — a failure here doesn't abort the pipeline since
/// RAG export (JSONL) already succeeded.
pub fn save_files(
    results: &[ScrapedContent],
    output_dir: &Path,
    format: &OutputFormat,
    obsidian_options: &ObsidianOptions,
) {
    if let Err(e) = save_results(results, output_dir, format, obsidian_options) {
        warn!(error = %e, "failed to save individual output files");
        // Continue — file save is non-fatal, RAG export succeeded
    }
}

// The AI export path is ungated (ADR-0004 paso 6): the cleaner runs over
// whatever `EmbeddingPort` the run wired, and the mock cleaner below only
// needs the core `SemanticCleaner` trait, which no longer depends on ONNX.
#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use crate::cli::error::CliExit;
    use crate::domain::semantic_cleaner::{private::Sealed, SemanticCleaner};
    use crate::domain::value_objects::ValidUrl;
    use crate::domain::DocumentChunk;
    use crate::domain::ScrapedContent;
    use crate::error::SemanticError;

    use super::clean_all_pages;

    /// Mock cleaner that always fails — used to assert total-failure propagation.
    struct FailingCleaner;

    impl Sealed for FailingCleaner {}

    impl SemanticCleaner for FailingCleaner {
        fn clean<'a>(
            &'a self,
            _url: &'a str,
            _html: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>>
        {
            Box::pin(async move {
                Err(SemanticError::Inference(
                    "mock cleaner always fails (#543 regression)".into(),
                ))
            })
        }

        fn max_chars(&self) -> usize {
            512
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    fn sample_result(url: &str, html: &str) -> ScrapedContent {
        ScrapedContent {
            title: String::new(),
            content: String::new(),
            url: ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: Some(html.to_string()),
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    /// #543 regression: when EVERY page fails to clean, the pipeline must return
    /// an error (non-zero exit) instead of silently exiting 0 with raw fallback.
    ///
    /// With the ErrorClass fail-fast design (#581 follow-up), an infrastructure-
    /// fatal error (Inference/ModelLoad) aborts the whole crawl on the FIRST
    /// page instead of processing all pages just to discover they all fail.
    #[tokio::test]
    async fn test_clean_all_pages_total_failure_propagates_error() {
        let cleaner: Arc<dyn SemanticCleaner> = Arc::new(FailingCleaner);
        let results = vec![
            sample_result("https://example.com/a", "<p>a</p>"),
            sample_result("https://example.com/b", "<p>b</p>"),
        ];

        let outcome = clean_all_pages(&results, &cleaner, None).await;

        // Inference error is InternalFatal → fail-fast: abort immediately with
        // a config error instead of waiting for all pages to fail.
        match outcome {
            Err(CliExit::ConfigError(msg)) => {
                assert!(
                    msg.contains("fatal") || msg.contains("infraestructura"),
                    "error should indicate infrastructure failure: {msg}"
                );
            },
            other => panic!(
                "expected Err(CliExit::ConfigError) for infrastructure failure, got {:?}",
                std::mem::discriminant(&other)
            ),
        }
    }

    /// Build the chunk a healthy page would produce, with a non-empty body.
    fn cleaned_chunk(url: &str, html: &str) -> DocumentChunk {
        let mut page = sample_result(url, html);
        page.content = html.to_string();
        DocumentChunk::from_scraped_content(&page)
    }

    /// Cleaner whose per-page behavior is scripted by the test: URLs carrying
    /// `slow` never resolve (a remote embedding call still in flight when the
    /// signal arrives), every other page returns one chunk immediately.
    struct ScriptedCleaner {
        hang_slow: bool,
    }

    impl Sealed for ScriptedCleaner {}

    impl SemanticCleaner for ScriptedCleaner {
        fn clean<'a>(
            &'a self,
            url: &'a str,
            html: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<DocumentChunk>, SemanticError>> + Send + 'a>>
        {
            let hang = self.hang_slow && url.contains("slow");
            Box::pin(async move {
                if hang {
                    std::future::pending::<()>().await;
                }
                Ok(vec![cleaned_chunk(url, html)])
            })
        }

        fn max_chars(&self) -> usize {
            512
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    /// #2270: a cancelled token stops the cleaning at the NEXT page and keeps
    /// the work that already finished. The page still in flight is dropped
    /// instead of awaited, so the run cannot keep paying for embeddings the
    /// user already asked it to stop requesting.
    #[tokio::test]
    async fn test_clean_all_pages_stops_at_the_next_page_when_cancelled() {
        let cleaner: Arc<dyn SemanticCleaner> = Arc::new(ScriptedCleaner { hang_slow: true });
        let results = vec![
            sample_result("https://example.com/fast", "<p>fast</p>"),
            sample_result("https://example.com/slow", "<p>slow</p>"),
        ];
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();

        let outcome = clean_all_pages(&results, &cleaner, Some(&token))
            .await
            .expect("a cancelled cleaning is not an error");

        assert_eq!(
            outcome.len(),
            1,
            "only the page that already finished may survive the drain"
        );
        assert!(
            outcome[0].url.ends_with("/fast"),
            "the surviving chunk must be the completed page, got {}",
            outcome[0].url
        );
    }

    /// Regression guard for the stream rewrite: WITHOUT a token every page is
    /// still cleaned and folded — cancellation must not swallow work.
    #[tokio::test]
    async fn test_clean_all_pages_without_token_cleans_every_page() {
        let cleaner: Arc<dyn SemanticCleaner> = Arc::new(ScriptedCleaner { hang_slow: false });
        let results = vec![
            sample_result("https://example.com/one", "<p>one</p>"),
            sample_result("https://example.com/two", "<p>two</p>"),
        ];

        let outcome = clean_all_pages(&results, &cleaner, None)
            .await
            .expect("an uncancelled cleaning succeeds");

        assert_eq!(outcome.len(), 2, "every page must be folded exactly once");
    }
}

#[cfg(test)]
mod standard_export_dispatch {
    //! Behavior-preservation pins for the blocking-pool dispatch
    //! (issue #1814, slice B AC4): moving `process_results` onto the blocking
    //! pool must not change what the export writes or reports. The off-runtime
    //! scheduling property itself is proven by the starvation tests in
    //! `llm_wire.rs` and the MCP export handler suite.

    use std::sync::Arc;

    use super::{run_standard_export, ExportConfig};
    use crate::domain::value_objects::ValidUrl;
    use crate::domain::ScrapedContent;

    fn scraped(url: &str, title: &str, body: &str) -> ScrapedContent {
        ScrapedContent {
            title: title.to_string(),
            content: body.to_string(),
            url: ValidUrl::parse(url).expect("valid test url"),
            excerpt: None,
            author: None,
            date: None,
            html: None,
            assets: Vec::new(),
            correlation_id: None,
            quality_hint: None,
        }
    }

    fn config_for(
        results: Vec<ScrapedContent>,
        output_dir: std::path::PathBuf,
    ) -> ExportConfig<'static> {
        ExportConfig {
            results: Arc::from(results),
            output_dir,
            format: crate::OutputFormat::default(),
            export_format: crate::ExportFormat::Jsonl,
            clean_ai: false,
            quick_save: false,
            vault_path: None,
            obsidian_options: Default::default(),
            state_store: None,
            resume: false,
            cancel: None,
            ai_threshold: 0.5,
            ai_max_chars: 512,
            ai_offline: false,
        }
    }

    #[tokio::test]
    async fn run_standard_export_writes_jsonl_and_reports_urls() {
        let tmp = tempfile::TempDir::new().expect("tmpdir");
        let config = config_for(
            vec![
                scraped("https://example.com/a", "A", "body a"),
                scraped("https://example.com/b", "B", "body b"),
            ],
            tmp.path().to_path_buf(),
        );

        let urls = run_standard_export(config).await.expect("export succeeds");

        assert_eq!(urls.len(), 2, "both items must be reported as processed");
        let body =
            std::fs::read_to_string(tmp.path().join("export.jsonl")).expect("jsonl is written");
        assert_eq!(
            body.lines().filter(|l| !l.trim().is_empty()).count(),
            2,
            "both items must reach the export file"
        );
    }
}
