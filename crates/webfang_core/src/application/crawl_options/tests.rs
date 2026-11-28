use super::*;

#[test]
fn test_default_crawl_limits() {
    let limits = CrawlLimits::default();
    assert_eq!(limits.selector, "body");
    assert_eq!(limits.max_depth, 2);
    assert_eq!(limits.max_pages, 10);
    assert!(!limits.single_page);
    assert!(limits.include_patterns.is_empty());
    assert!(limits.exclude_patterns.is_empty());
    assert!(!limits.interactive);
    assert!(!limits.resume);
    assert!(limits.state_dir.is_none());
    assert!(!limits.use_sitemap);
    assert!(limits.sitemap_url.is_none());
    assert_eq!(limits.checkpoint_interval, 100);
    assert!(!limits.no_checkpoint);
    assert!(!limits.ignore_robots);
    assert!(!limits.no_session_health);
    assert!(!limits.ignore_waf);
    assert!(limits.dom_preprune, "dom_preprune defaults to true");
}

#[test]
fn test_default_network_options() {
    let net = NetworkOptions::default();
    assert!(net.user_agent.is_none());
    assert_eq!(net.accept_language, "en-US,en;q=0.9");
    assert!(net.concurrency.is_auto());
    assert_eq!(net.delay_ms, 1000);
    assert_eq!(net.timeout_secs, 30);
    assert_eq!(net.max_retries, 3);
    assert_eq!(net.backoff_base_ms, 1000);
    assert_eq!(net.backoff_max_ms, 10000);
    assert!(!net.download_images);
    assert!(!net.download_documents);
    assert_eq!(net.h2_profile, "Chrome145");
    assert_eq!(net.js_strategy, JsStrategy::Static);
    assert_eq!(net.obscura_binary, "obscura");
}

#[test]
fn test_default_export_options() {
    let export = ExportOptions::default();
    assert_eq!(export.output_format, OutputFormat::Markdown);
    assert_eq!(export.export_format, ExportFormat::Jsonl);
    assert_eq!(export.output_dir, PathBuf::from("output"));
    assert!(!export.dry_run);
    assert!(!export.quiet);
    assert!(export.obsidian_vault.is_none());
    assert!(!export.vault_is_explicit);
    assert!(!export.obsidian_rich_metadata);
    assert!(export.obsidian_tags.is_empty());
    assert!(!export.obsidian_wiki_links);
    assert!(!export.obsidian_relative_assets);
    assert!(!export.quick_save);
}

#[test]
fn test_default_ingestion_tuning() {
    let tuning = IngestionTuning::default();
    assert!(!tuning.enabled);
    assert!(tuning.cpu_cores.is_none());
    assert!(tuning.ram_budget_bytes.is_none());
    assert!(tuning.db_path.is_none());
    assert!(tuning.max_resource_bytes.is_none());
}

#[test]
fn test_default_crawl_options() {
    let opts = CrawlOptions::default();
    assert_eq!(opts.url.as_str(), "https://example.com/");
    assert_eq!(opts.verbosity, 0);
    assert!(!opts.quiet);
    assert_eq!(opts.asset_naming, "hash");
}

#[test]
fn test_from_elastic_overrides() {
    let overrides = ElasticOverrides {
        cpu_cores: Some(4),
        ram_budget_bytes: Some(8 * 1024 * 1024 * 1024),
        max_resource_bytes: Some(25 * 1024 * 1024),
        db_path: Some(PathBuf::from("/tmp/test.db")),
    };
    let tuning = IngestionTuning::from(overrides);
    assert!(tuning.enabled);
    assert_eq!(tuning.cpu_cores, Some(4));
    assert_eq!(tuning.ram_budget_bytes, Some(8 * 1024 * 1024 * 1024));
    assert_eq!(tuning.max_resource_bytes, Some(25 * 1024 * 1024));
    assert_eq!(tuning.db_path, Some(PathBuf::from("/tmp/test.db")));
}

#[test]
fn test_from_elastic_overrides_empty() {
    let overrides = ElasticOverrides::default();
    let tuning = IngestionTuning::from(overrides);
    assert!(tuning.enabled);
    assert!(tuning.cpu_cores.is_none());
    assert!(tuning.ram_budget_bytes.is_none());
    assert!(tuning.max_resource_bytes.is_none());
    assert!(tuning.db_path.is_none());
}

#[test]
fn test_ai_config_defaults() {
    let config = AiConfig::default();
    assert_eq!(config.threshold, 0.3);
    assert_eq!(config.max_chars, 98_304);
    assert!(!config.offline);
    assert!(
        !config.strict_clean_ai,
        "the compatibility default must NOT fail a partially degraded run"
    );
}

#[test]
fn test_ai_config_custom_values() {
    let config = AiConfig {
        threshold: 0.7,
        max_chars: 2048,
        offline: true,
        strict_clean_ai: true,
    };
    assert_eq!(config.threshold, 0.7);
    assert_eq!(config.max_chars, 2048);
    assert!(config.offline);
    assert!(config.strict_clean_ai);
}

#[test]
fn test_crawl_options_has_ai_config_field() {
    let opts = CrawlOptions::default();
    let ai = opts.ai_config;
    assert_eq!(ai.threshold, 0.3);
    assert_eq!(ai.max_chars, 98_304);
    assert!(!ai.offline);
}

#[test]
fn test_ai_config_is_clone() {
    let config = AiConfig {
        threshold: 0.5,
        max_chars: 1024,
        offline: true,
        strict_clean_ai: false,
    };
    let cloned = config.clone();
    assert_eq!(config, cloned);
}
