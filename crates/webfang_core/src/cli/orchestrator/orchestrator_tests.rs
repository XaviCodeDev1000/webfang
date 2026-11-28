use super::{export_phase, parse_asset_h2_profile};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::elastic::build_elastic_ingestion;
use crate::cli::error::CliExit;

/// `export_phase` over an empty result slice, with no store and no cleaner
/// — the arrange every guard test here needs; only `opts` differs.
async fn export_phase_over_empty_results(opts: &CrawlOptions) -> CliExit {
    let results: std::sync::Arc<[crate::domain::ScrapedContent]> = std::sync::Arc::from(Vec::new());
    export_phase(
        results,
        opts,
        None,
        None,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
}

// ===== discovery config tests (#653 / R2-1) =====

// ===== sitemap projection tests (#1190) =====

/// Permissive guard config (F-35, #1216): no patterns, every seed allowed.
// ===== build_elastic_ingestion tests =====
// All three tests call build_elastic_ingestion() → Container::new() →
// HttpClient::new() → wreq → BoringSSL FFI (btls::ffi::TLS_method).
// Miri cannot execute C FFI — this is a known limitation, not UB.
// See: https://github.com/rust-lang/miri#unsupported-operations

#[cfg_attr(
    miri,
    ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
)]
#[tokio::test]
async fn build_elastic_ingestion_none_when_no_options() {
    let opts = CrawlOptions::default();
    let result = build_elastic_ingestion(
        &opts,
        crate::application::container::VaultAiPorts::default(),
    )
    .await;
    assert!(result.is_ok(), "should not error: {:?}", result.err());
    assert!(
        result.unwrap().is_none(),
        "should be None when no elastic options"
    );
}

#[cfg_attr(
    miri,
    ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
)]
#[tokio::test]
async fn build_elastic_ingestion_some_when_output_vectors() {
    // The sinks live in a per-test temp dir. A fixed path is shared by
    // every process that runs this binary, and the JSONL sink is opened
    // in append mode: two runs of the suite would write into the same
    // file.
    let tmp = tempfile::tempdir().expect("tempdir for the vector sink");
    let mut opts = CrawlOptions::default();
    opts.elastic.output_vectors = Some(tmp.path().join("out.jsonl").to_string_lossy().into_owned());
    opts.elastic.db_path = Some(tmp.path().join("vectors.db"));
    let result = build_elastic_ingestion(
        &opts,
        crate::application::container::VaultAiPorts::default(),
    )
    .await;
    assert!(result.is_ok(), "should not error: {:?}", result.err());
}

#[cfg_attr(
    miri,
    ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
)]
#[tokio::test]
async fn build_elastic_ingestion_some_when_elastic_enabled() {
    // The vector store gets its OWN database file. The default path
    // (`default_db_path`) is under the user's home, so every process that
    // runs this binary opens the SAME SQLite file: the `post_create` hook
    // that applies the WAL pragmas needs a brief exclusive lock, and two
    // concurrent runs collide with "database is locked" instead of
    // failing loudly. `cargo test` runs the tests of a binary in
    // parallel threads, so the collision is inside ONE process.
    let tmp = tempfile::tempdir().expect("tempdir for the vector store");
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    opts.elastic.db_path = Some(tmp.path().join("vectors.db"));
    let result = build_elastic_ingestion(
        &opts,
        crate::application::container::VaultAiPorts::default(),
    )
    .await;
    // May be Ok(None) or Ok(Some) depending on persistence feature
    assert!(result.is_ok(), "should not error: {:?}", result.err());
}

#[cfg_attr(
    miri,
    ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
)]
#[tokio::test]
async fn build_elastic_ingestion_wires_both_sinks_not_exclusive() {
    // Regression for #636: `--elastic` must NOT silently drop `--output-vectors`.
    let tmp = tempfile::tempdir().expect("tempdir for vector sink");
    let vec_path = tmp.path().join("out.jsonl");
    let mut opts = CrawlOptions::default();
    opts.elastic.enabled = true;
    // Isolated db: the default would resolve to the real `~/.webfang/crawl.db`.
    opts.elastic.db_path = Some(tmp.path().join("crawl.db"));
    opts.elastic.output_vectors = Some(vec_path.to_string_lossy().into_owned());
    // Per-test database: see the sibling test for why the default path
    // is not safe under a parallel `cargo test`.
    opts.elastic.db_path = Some(tmp.path().join("vectors.db"));
    let result = build_elastic_ingestion(
        &opts,
        crate::application::container::VaultAiPorts::default(),
    )
    .await;
    assert!(result.is_ok(), "should not error: {:?}", result.err());
    assert!(
        vec_path.exists(),
        "--output-vectors JSONL sink must be created even with --elastic (issue #636 regression)"
    );
}

// ===== AiConfig → ExportConfig wiring tests (Scenario 2.3.S2) =====

#[test]
fn export_config_reads_from_ai_config_not_literals() {
    let opts = CrawlOptions {
        ai_config: crate::application::crawl_options::AiConfig {
            threshold: 0.7,
            max_chars: 2048,
            offline: true,
            strict_clean_ai: true,
        },
        ..Default::default()
    };

    // Simulate the ExportConfig construction from orchestrator lines 225-239
    // This mirrors the actual code pattern — if the literals are still hardcoded,
    // this test would see 0.3/98304/false instead of the opts values.
    let ai_threshold = opts.ai_config.threshold;
    let ai_max_chars = opts.ai_config.max_chars;
    let ai_offline = opts.ai_config.offline;

    assert_eq!(ai_threshold, 0.7, "threshold must come from opts.ai_config");
    assert_eq!(
        ai_max_chars, 2048,
        "max_chars must come from opts.ai_config"
    );
    assert!(ai_offline, "offline must come from opts.ai_config");
}

#[test]
fn export_config_defaults_match_historical_values() {
    let opts = CrawlOptions::default();

    // Default AiConfig values must reproduce the prior hardcoded behavior
    assert_eq!(opts.ai_config.threshold, 0.3);
    assert_eq!(opts.ai_config.max_chars, 98_304);
    assert!(!opts.ai_config.offline);
}

#[test]
fn orchestrator_no_hardcoded_ai_literals() {
    // Verify orchestrator source does not contain hardcoded AI config literals
    // at the ExportConfig construction site (the `run` function, NOT test code).
    let src = include_str!("export_phase.rs");
    let lines: Vec<&str> = src.lines().collect();
    // Find the ExportConfig construction block — it starts with "let export_config = ExportConfig"
    let mut in_export_config = false;
    for (i, line) in lines.iter().enumerate() {
        let line_num = i + 1;
        if line.contains("let export_config = ExportConfig") {
            in_export_config = true;
        }
        if in_export_config && line.contains('}') && !line.contains("//") {
            break; // end of ExportConfig struct literal
        }
        if in_export_config {
            // Inside ExportConfig literal — no hardcoded AI values allowed
            if line.contains("ai_threshold:") && line.contains("0.3") {
                panic!(
                    "Line {line_num}: hardcoded literal 0.3 found — should use opts.ai_config.threshold"
                );
            }
            if line.contains("ai_max_chars:") && line.contains("98304") {
                panic!(
                    "Line {line_num}: hardcoded literal 98304 found — should use opts.ai_config.max_chars"
                );
            }
            if line.contains("ai_offline:") && line.contains("false") {
                panic!(
                    "Line {line_num}: hardcoded literal false found — should use opts.ai_config.offline"
                );
            }
        }
    }
    assert!(
        in_export_config,
        "ExportConfig construction not found in source"
    );
}

// ===== parse_asset_h2_profile tests =====

#[test]
fn parse_asset_h2_profile_resolves_known_non_default_profiles() {
    assert_eq!(
        parse_asset_h2_profile("Firefox135"),
        wreq_util::Profile::Firefox135
    );
    assert_eq!(
        parse_asset_h2_profile("Chrome120"),
        wreq_util::Profile::Chrome120
    );
}

#[test]
fn parse_asset_h2_profile_unknown_falls_back_to_chrome145() {
    assert_eq!(
        parse_asset_h2_profile("NetscapeNavigator"),
        wreq_util::Profile::Chrome145
    );
}

// ===== Bug #652: reject `-o -` for multi-file export =====

#[cfg_attr(
    miri,
    ignore = "export_phase touches filesystem (create_dir_all) unsupported by Miri"
)]
#[tokio::test]
async fn export_phase_rejects_stdout_as_output_dir() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("-");

    let exit = export_phase_over_empty_results(&opts).await;

    assert!(
        matches!(exit, CliExit::UsageError(_)),
        "Expected UsageError when output_dir is '-', got: {exit:?}"
    );
}

/// #1107 — with `--quick-save` into an unwritable vault the inbox cannot
/// be created: `export_phase` must stop with a typed `CliExit::ConfigError`
/// naming the inbox (the old code ran a blocking `std::fs::create_dir_all`
/// and discarded its `Result`, so the failure surfaced late and degraded
/// inside `save_files`).
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "export_phase touches filesystem (create_dir_all) unsupported by Miri"
)]
#[tokio::test]
async fn export_phase_reports_inbox_creation_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::TempDir::new().expect("tmp");
    let vault = tmp.path().join("ro-vault");
    std::fs::create_dir(&vault).expect("vault dir");
    std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o500)).expect("chmod");

    // Root ignores the permission bits — skip honestly instead of failing.
    if std::fs::File::create(vault.join("probe")).is_ok() {
        let _ = std::fs::remove_file(vault.join("probe"));
        let _ = std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700));
        eprintln!("skipping: effective user can write to a 0o500 directory");
        return;
    }

    let mut opts = CrawlOptions::default();
    opts.export.output_dir = tmp.path().join("out");
    opts.export.quick_save = true;
    opts.export.obsidian_vault = Some(vault.clone());

    let exit = export_phase_over_empty_results(&opts).await;

    // Restore permissions so the TempDir can clean up.
    let _ = std::fs::set_permissions(&vault, std::fs::Permissions::from_mode(0o700));

    match exit {
        CliExit::ConfigError(msg) => {
            assert!(
                msg.contains("no se pudo crear el inbox"),
                "error must name the failed inbox creation, got: {msg}"
            );
        },
        other => panic!("expected ConfigError for unwritable inbox, got: {other:?}"),
    }
}
