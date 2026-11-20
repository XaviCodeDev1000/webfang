use super::*;

#[test]
fn test_pipeline_output_format_default() {
    let format = PipelineOutputFormat::default();
    assert_eq!(format, PipelineOutputFormat::Jsonl);
}

#[test]
fn test_pipeline_output_format_variants() {
    let jsonl = PipelineOutputFormat::Jsonl;
    let none = PipelineOutputFormat::None;
    assert_ne!(jsonl, none);
}

#[test]
fn test_output_format_default() {
    let format = OutputFormat::default();
    assert_eq!(format, OutputFormat::Markdown);
}

#[test]
fn test_output_format_variants() {
    let md = OutputFormat::Markdown;
    let json = OutputFormat::Json;
    let text = OutputFormat::Text;
    assert_ne!(md, json);
    assert_ne!(md, text);
    assert_ne!(json, text);
}

#[test]
fn test_export_format_default() {
    let format = ExportFormat::default();
    assert_eq!(format, ExportFormat::Jsonl);
}

#[test]
fn test_export_format_variants() {
    let jsonl = ExportFormat::Jsonl;
    let vector = ExportFormat::Vector;
    let auto = ExportFormat::Auto;
    assert_ne!(jsonl, vector);
    assert_ne!(jsonl, auto);
    assert_ne!(vector, auto);
}

#[test]
fn test_concurrency_config_default_is_auto() {
    let config = ConcurrencyConfig::default();
    assert!(config.is_auto());
}

#[test]
fn test_concurrency_config_new_explicit() {
    let config = ConcurrencyConfig::new(5);
    assert!(!config.is_auto());
    assert_eq!(config.resolve(), 5);
}

#[test]
fn test_concurrency_config_clamps() {
    let config = ConcurrencyConfig::new(100);
    assert_eq!(config.resolve(), 16);

    let config = ConcurrencyConfig::new(0);
    assert_eq!(config.resolve(), 1);
}

#[test]
fn test_concurrency_config_display() {
    let auto = ConcurrencyConfig::auto();
    assert_eq!(format!("{auto}"), "auto");

    let explicit = ConcurrencyConfig::new(5);
    assert_eq!(format!("{explicit}"), "5");
}

/// Zero Silent Loss (P4-2, #1296): the argv-boundary parser must reject
/// `0` instead of silently clamping it to 1.
#[test]
fn concurrency_value_parser_rejects_zero() {
    use clap::builder::TypedValueParser as _;
    let err = ConcurrencyValueParser
        .parse_ref(&clap::Command::new("test"), None, std::ffi::OsStr::new("0"))
        .expect_err("zero concurrency must be rejected at the argv boundary");
    assert!(
        err.to_string().contains("--concurrency debe ser >= 1"),
        "got: {err}"
    );
}

#[test]
fn test_concurrency_config_from_str() {
    let config = ConcurrencyConfig::from("5");
    assert_eq!(config.resolve(), 5);

    let config = ConcurrencyConfig::from("auto");
    assert!(config.is_auto());

    let config = ConcurrencyConfig::from("");
    assert!(config.is_auto());
}

#[test]
fn test_concurrency_config_from_str_invalid() {
    let config = ConcurrencyConfig::from("not-a-number");
    assert!(config.is_auto());
}

/// Characterization sweep: every explicit value must produce exactly
/// what the canonical budget clamp produces (single-source invariant).
#[test]
fn explicit_values_match_canonical_clamp_sweep() {
    use std::num::NonZeroUsize;

    let one = NonZeroUsize::new(1).expect("1 is non-zero");
    for value in [0, 1, 2, 3, 15, 16, 17, 100, usize::MAX] {
        let expected = crate::domain::budget::clamp::clamp_budget(
            value,
            one,
            crate::domain::budget::clamp::MAX_CONCURRENCY_CEILING,
        );
        assert_eq!(
            ConcurrencyConfig::new(value).resolve(),
            expected.get(),
            "explicit {value} diverges from canonical clamp"
        );
    }
}

/// Characterization of the auto-detection table: reference impl of
/// TODAY'S `resolve()` math (1-2→1, 3-4→3, 5-7→5, else min(cores−1, 8)).
/// Guards the "auto" path while the clamp sites delegate.
#[test]
fn auto_path_matches_legacy_table() {
    // #897 item 3: derive from the canonical hardware seam instead of
    // calling `available_parallelism` directly, so "auto" resolves
    // identically in every subsystem (cgroup limits included).
    let cores = crate::domain::budget::detector::system_parallelism().get();
    let expected = match cores {
        1 | 2 => 1,
        3 | 4 => 3,
        5..=7 => 5,
        _ => (cores - 1).min(8),
    }
    .clamp(1, 16);
    assert_eq!(ConcurrencyConfig::default().resolve(), expected);
}

// === TDD RED for 990 task 1.2: ScraperConfig must live in domain::config ===
#[test]
fn scraper_config_lives_in_domain() {
    // RED: this test references ScraperConfig that does NOT yet exist in
    // domain::config — compile should fail until GREEN moves it.
    let cfg = crate::domain::config::ScraperConfig::default();
    assert!(!cfg.download_images);
    assert_eq!(cfg.scraper_concurrency, 3);
}

#[test]
fn scraper_config_builder_in_domain() {
    let cfg = crate::domain::config::ScraperConfig::default()
        .with_images()
        .with_scraper_concurrency(5);
    assert!(cfg.download_images);
    assert_eq!(cfg.scraper_concurrency, 5);
}

// TDD RED for 990 task 2.1: downloader_port must exist in domain
#[test]
fn downloader_port_trait_exists() {
    let err = crate::domain::downloader_port::DownloadError::timeout(5);
    assert!(err.to_string().contains("timed out"));
}

// TDD RED for 990 task 2.2: crawler_port SitemapConfig must exist
#[test]
fn crawler_port_sitemap_exists() {
    let cfg = crate::domain::crawler_port::SitemapConfig::default();
    assert_eq!(cfg.max_depth, 3);
    assert_eq!(cfg.concurrency, 5);
}
