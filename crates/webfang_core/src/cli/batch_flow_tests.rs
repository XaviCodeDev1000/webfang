use super::batch_manager::build_batch_crawler_config;
use super::run_batch;
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use crate::CrawlerConfig;

/// Build the batch config projection for `opts` the way preflight does:
/// the explicit overrides ride through a freshly built [`BudgetModel`].
fn projected_config(opts: &CrawlOptions) -> CrawlerConfig {
    let budget = crate::domain::budget::BudgetModel::build(
        opts.budget_overrides,
        &crate::domain::budget::detector::SystemDetector,
    );
    build_batch_crawler_config(opts, wreq_util::Profile::Chrome145, &budget)
        .expect("valid test projection must build")
}

#[test]
fn batch_config_propagates_delay_and_model_concurrency() {
    // Regression for #653: per-URL rate limiting never engaged on the
    // batch path. The concurrency bound now derives from the run's
    // BudgetModel crawl tier; an explicit `--concurrency` value reaches
    // it THROUGH the model (explicit-wins override, design D4).
    let mut opts = CrawlOptions::default();
    opts.network.delay_ms = 750;
    // Explicit flag feeds the model override exactly as preflight does.
    opts.network.concurrency = crate::ConcurrencyConfig::new(2);
    if let Some(explicit) = opts.network.concurrency.get() {
        opts.budget_overrides.crawl =
            crate::domain::budget::tiers::CrawlConcurrency::new(explicit).ok();
    }

    let config = projected_config(&opts);

    assert_eq!(config.delay_ms, 750, "--delay-ms must reach the crawler");
    assert_eq!(
        config.concurrency.get(),
        2,
        "explicit --concurrency must reach the crawler through the model"
    );
}

#[test]
fn batch_config_propagates_budget_overrides() {
    // Bug R2-1: --batch crawls each URL through the crawl Engine via
    // BatchProcessor.process_single_url -> crawl_site; the operator
    // overrides must ride on the base config handed to the batch job.
    let mut opts = CrawlOptions::default();
    opts.budget_overrides.crawl = crate::domain::budget::tiers::CrawlConcurrency::new(4).ok();
    opts.budget_overrides.rate_burst = crate::domain::budget::tiers::BurstPermits::new(13).ok();

    let config = projected_config(&opts);

    assert_eq!(
        config.budget_overrides.crawl.map(|c| c.get()),
        Some(4),
        "explicit --concurrency must reach the batch Engine"
    );
    assert_eq!(
        config.budget_overrides.rate_burst.map(|b| b.get()),
        Some(13),
        "explicit --rate-limit-burst must reach the batch Engine"
    );
}

#[test]
fn batch_config_auto_concurrency_uses_model_tier() {
    // With no explicit flag, the model's auto-derived crawl tier is used.
    let opts = CrawlOptions::default();
    assert!(opts.network.concurrency.is_auto());
    let budget = crate::domain::budget::BudgetModel::for_test_preset();

    let config = build_batch_crawler_config(&opts, wreq_util::Profile::Chrome145, &budget)
        .expect("valid test projection must build");

    assert_eq!(
        config.concurrency.get(),
        budget.crawl().get(),
        "auto mode must use the model's derived Operation.crawl tier"
    );
}

#[test]
fn batch_config_zero_delay_disables_throttling() {
    let mut opts = CrawlOptions::default();
    opts.network.delay_ms = 0;

    let config = build_batch_crawler_config(
        &opts,
        wreq_util::Profile::Chrome145,
        &crate::domain::budget::BudgetModel::for_test_preset(),
    )
    .expect("valid test projection must build");

    assert_eq!(config.delay_ms, 0);
}

#[tokio::test]
async fn run_batch_returns_data_error_when_output_vectors_without_clean_ai() {
    let mut opts = CrawlOptions::default();
    opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
    let cancel = tokio_util::sync::CancellationToken::new();

    let exit = run_batch(
        opts,
        None,
        crate::application::container::VaultAiPorts::default(),
        None,
        &cancel,
        &crate::domain::CorrelationId::new(),
    )
    .await;

    assert!(
        matches!(exit, CliExit::DataFormatError(_)),
        "expected CliExit::DataFormatError, got {exit:?}"
    );
}
