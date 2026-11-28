//! Behavior-preservation pins for the blocking-pool dispatch
//! (issue #1814, slice B AC4): moving `process_results` onto the blocking
//! pool must not change what the export writes or reports. The off-runtime
//! scheduling property itself is proven by the starvation tests in
//! `llm_wire.rs` and the MCP export handler suite.

use std::sync::Arc;

use super::{run_standard_export, test_scraped as scraped, ExportConfig};
use crate::domain::ScrapedContent;

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
        ai_strict: false,
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
    let body = std::fs::read_to_string(tmp.path().join("export.jsonl")).expect("jsonl is written");
    assert_eq!(
        body.lines().filter(|l| !l.trim().is_empty()).count(),
        2,
        "both items must reach the export file"
    );
}
