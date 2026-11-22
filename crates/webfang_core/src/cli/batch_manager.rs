//! Batch manager assembly — load, configure, filter and validate the `--batch`
//! inputs.
//!
//! Extracted from `cli/batch_flow.rs` (Ola 3 B4, issue #2966): everything that
//! builds the [`BatchManager`] — file/stdin loading (`--batch-file` and the
//! dry-run preview both reach [`load_batch_manager`]), the batch crawl-config
//! projection (`--h2-profile`, delay/concurrency through the budget model),
//! the resume COMMITTED filter (WU4) and the inert crawl-expansion-flag
//! warning (#1215). The run lifecycle stays in the parent, and so does
//! `build_batch_resume_store` — both seams call it. The batch-config
//! regression tests live in the parent's `batch_flow_tests.rs`.

use tracing::{error, info, warn};

use super::build_batch_resume_store;
use crate::application::batch::BatchManager;
use crate::application::crawl_options::{CrawlLimits, CrawlOptions};
use crate::application::crawler::BoundedFileSink;
use crate::cli::batch_summary::{enforce_strict_input, input_accounting};
use crate::cli::discovery_phase::base_crawler_config_builder;
use crate::cli::error::CliExit;
use crate::domain;
use crate::domain::http_config::HttpClientConfig;
use crate::CrawlerConfig;

/// Warn when crawl-expansion flags are set on a `--batch` run (#1215).
///
/// Batch scrapes each input URL (one page per URL), so `--max-depth`,
/// `--max-pages`, and `--sitemap` cannot expand anything here. A value that
/// differs from the default means the operator asked for a crawl — staying
/// silent would let them assume one happened — hence `warn!`, naming every
/// inert flag. Defaults are read from [`CrawlLimits::default`] (the same
/// source the CLI defaults mirror) rather than hardcoded, so the comparison
/// cannot rot when defaults move.
fn warn_batch_crawl_flags_ignored(opts: &CrawlOptions) {
    let defaults = CrawlLimits::default();
    let mut inert = Vec::new();
    if opts.crawl.max_depth != defaults.max_depth {
        inert.push(format!("--max-depth {}", opts.crawl.max_depth));
    }
    if opts.crawl.max_pages != defaults.max_pages {
        inert.push(format!("--max-pages {}", opts.crawl.max_pages));
    }
    if opts.crawl.use_sitemap || opts.crawl.sitemap_url.is_some() {
        inert.push("--sitemap".to_string());
    }
    if inert.is_empty() {
        info!("Batch mode scrapes each input URL (one page per URL, no crawling)");
    } else {
        warn!(
            flags = inert.join(", "),
            "--batch scrapes each input URL (one page per URL): crawl-expansion flags are ignored (#1215)"
        );
    }
}

/// Load the batch manager, attach the capture sink, and assert it has work.
///
/// Warns loudly when crawl-expansion flags (`--max-depth`, `--max-pages`,
/// `--sitemap`) differ from their defaults: since #1215 batch scrapes each
/// URL instead of crawling it, those flags are inert here and the operator
/// must hear about it rather than assume a crawl happened.
///
/// `root_correlation` (#1439) travels onto the [`BatchProcessor`](crate::application::batch::BatchProcessor) so every
/// per-URL crawl engine of this run shares the one announced identity.
pub(super) async fn prepare_batch_manager(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    sink: std::sync::Arc<BoundedFileSink>,
    root_correlation: &domain::CorrelationId,
) -> Result<BatchManager, CliExit> {
    warn_batch_crawl_flags_ignored(opts);
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    let manager = load_batch_manager(opts, crawler_config, &budget)
        .await?
        .with_content_sink(sink.clone())
        .with_correlation(root_correlation.clone());

    // WU4 (#2609/#2611): with `--resume`, drop the URLs whose records are
    // COMMITTED-proven in the batch store BEFORE any fetch — a relaunch must
    // not re-download pages the previous run already exported (S7 iv: zero
    // double downloads). Runs on the blocking pool: the store read takes
    // the flock and does file I/O.
    let had_urls_before_resume_filter = manager.url_count() > 0;
    let manager = if opts.crawl.resume {
        filter_committed_batch(manager, opts, sink, root_correlation).await?
    } else {
        manager
    };

    if manager.url_count() == 0 {
        if had_urls_before_resume_filter {
            // Every input URL is already COMMITTED: the batch has nothing to
            // do, and that is a SUCCESS (exit 0), not a usage error — same
            // contract as the crawl-classic resume (#2609).
            info!("resume: all batch URLs already processed, nothing pending");
        } else {
            // #2795: the error counts what the source yielded and why nothing
            // is runnable — a bare "no URLs" sent the operator to count lines
            // by hand. Exit stays 64: an input that yields no URL is a usage
            // error (matrix row 15), strict-or-not.
            error!("No URLs provided for batch processing");
            let accounting = manager
                .input_lines()
                .map(input_accounting)
                .unwrap_or_else(|| "sin líneas de entrada".to_string());
            return Err(CliExit::UsageError(format!(
                "No hay URLs para procesar ({accounting})"
            )));
        }
    }

    info!(
        "Starting batch processing: {} URLs, concurrency={}",
        manager.url_count(),
        budget.batch().get()
    );

    Ok(manager)
}

/// Filter the batch URL list down to the not-yet-COMMITTED ones (WU4).
///
/// Uses the SAME typed gate as the crawl-classic resume
/// ([`crate::application::resume::filter_committed`]) against the batch
/// store, so a relaunched `--resume` batch skips exactly the pages a
/// previous run committed — including pages a checkpoint flush committed
/// mid-run. All-skipped is NOT an error here: the run exits through the
/// normal empty-batch path, which reports success with zero work.
async fn filter_committed_batch(
    manager: BatchManager,
    opts: &CrawlOptions,
    sink: std::sync::Arc<BoundedFileSink>,
    root_correlation: &domain::CorrelationId,
) -> Result<BatchManager, CliExit> {
    let store = match build_batch_resume_store(opts) {
        Ok(Some(s)) => crate::cli::scrape_flow::record_store_bridge(&*s),
        _ => return Ok(manager),
    };
    let urls = manager.urls();
    let parsed: Vec<url::Url> = urls
        .iter()
        .filter_map(|u| url::Url::parse(u).ok())
        .collect();
    let total = parsed.len();
    let manager_urls = manager.urls();
    let (pending, _run_id) = {
        let store = store.clone();
        let parsed = parsed.clone();
        tokio::task::spawn_blocking(move || {
            crate::application::resume::filter_committed(
                parsed,
                &store as &dyn crate::domain::persistence::RecordStorePort,
            )
        })
        .await
        .map_err(|e| CliExit::IoError(format!("No se pudo leer el estado de resume: {e}")))?
    };
    let pending_set: std::collections::HashSet<String> =
        pending.iter().map(|u| u.as_str().to_string()).collect();
    let kept: Vec<String> = manager_urls
        .into_iter()
        .filter(|u| {
            url::Url::parse(u)
                .map(|p| pending_set.contains(p.as_str()))
                .unwrap_or(true)
        })
        .collect();
    let skipped = total.saturating_sub(kept.len());
    if skipped > 0 {
        info!(
            skipped,
            total, "resume: skipping URLs already COMMITTED in the batch store"
        );
    }
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    // Rebuild the manager over the pending URLs with a fresh config — the
    // crawler_config was consumed by load_batch_manager, so rebuild it.
    let tls_emulation = resolve_batch_tls_emulation(opts)?;
    let crawler_config = build_batch_crawler_config(opts, tls_emulation, &budget)?;
    Ok(
        BatchManager::from_urls(kept, crawler_config, budget.batch().nonzero())
            .with_content_sink(sink)
            .with_correlation(root_correlation.clone()),
    )
}

/// Load the batch manager from a file or stdin.
///
/// Also reached by the `--batch-file` dry-run preview (`run_dry_run`), which
/// lists the URLs a batch run would scrape without fetching them.
pub(crate) async fn load_batch_manager(
    opts: &CrawlOptions,
    crawler_config: CrawlerConfig,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<BatchManager, CliExit> {
    let manager = if let Some(ref path) = opts.batch.batch_file {
        info!("Reading URLs from file: {}", path.display());
        // #2355: the budget tier is already `NonZeroUsize` — hand the typed
        // value through instead of downgrading to `usize` and revalidating.
        BatchManager::from_file(path, crawler_config, budget.batch().nonzero()).map_err(|e| {
            error!(error = %e, "Failed to read URLs from file");
            CliExit::IoError(format!("Failed to read URLs from file: {e}"))
        })?
    } else {
        info!("Reading URLs from stdin");
        load_batch_manager_from_stdin(crawler_config, budget.batch().nonzero()).await?
    };
    enforce_strict_input(&manager, opts)?;
    Ok(manager)
}

/// Read batch URLs from stdin on a blocking thread.
async fn load_batch_manager_from_stdin(
    crawler_config: CrawlerConfig,
    concurrency: std::num::NonZeroUsize,
) -> Result<BatchManager, CliExit> {
    // spawn_blocking: stdin read is blocking I/O that must not run on the
    // Tokio async runtime thread pool — it would block other tasks.
    match tokio::task::spawn_blocking(move || BatchManager::from_stdin(crawler_config, concurrency))
        .await
    {
        Ok(result) => result.map_err(|e| {
            error!(error = %e, "Failed to read URLs from stdin");
            CliExit::IoError(format!("Failed to read URLs from stdin: {e}"))
        }),
        Err(join_err) => {
            error!(error = %join_err, "stdin read task panicked");
            Err(CliExit::IoError(format!("Failed to read URLs: {join_err}")))
        },
    }
}

/// Resolve the TLS/H2 fingerprint for the batch crawl engine.
///
/// An unknown profile is a config error (exit 78), matching the scrape phase —
/// never silently crawl with a wrong fingerprint (#312).
pub(super) fn resolve_batch_tls_emulation(
    opts: &CrawlOptions,
) -> Result<wreq_util::Profile, CliExit> {
    HttpClientConfig::profile_from_name(&opts.network.h2_profile)
        .map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Build the crawler config for the batch engine, honoring `--h2-profile`.
///
/// `--delay-ms` and the concurrency bound are propagated here (#653): without
/// them the batch engine crawled at full speed with its own default concurrency,
/// making both flags silent no-ops on the `--batch` path.
///
/// #1215: the `max_pages`/`max_depth` values carried here are the CLI crawl
/// budget, but they never expand batch seeds — the processor rebuilds a
/// seed-only config per URL (the batch processor pins depth 0 / 1 page),
/// so every seed is scraped exactly once and `--max-pages 1` can no longer
/// starve seeds.
///
/// The concurrency bound comes from the run's [`BudgetModel`](crate::domain::budget::BudgetModel) Operation.crawl
/// tier (task 2.5b).
pub(super) fn build_batch_crawler_config(
    opts: &CrawlOptions,
    tls_emulation: wreq_util::Profile,
    budget: &crate::domain::budget::BudgetModel,
) -> Result<CrawlerConfig, CliExit> {
    Ok(base_crawler_config_builder(opts)?
        // Concurrency bound derives from the run's budget model
        // Operation.crawl tier (task 2.5b), not from the raw CLI flag.
        .concurrency(budget.crawl().nonzero())
        .tls_emulation(tls_emulation)
        .build())
}
