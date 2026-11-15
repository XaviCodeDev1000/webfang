//! `process_results` behavior tests: URL domain extraction, output
//! shaping, dedupe identity, filename containment, format files.

use super::tests_common::*;
use super::*;
use crate::domain::entities::ScrapedContent;
use tempfile::TempDir;

#[test]
fn test_domain_from_url_extracts_correctly() {
    let url = "https://www.example.com/docs/api/";
    let domain = domain_from_url(url);
    assert_eq!(domain, "www.example.com");
}

#[test]
fn test_domain_from_url_invalid_url_returns_unknown() {
    let domain = domain_from_url("not-a-url");
    assert_eq!(domain, "unknown");
}

// =========================================================================
// process_results tests (T2.2)
// =========================================================================

#[test]
fn test_process_results_empty_results_returns_empty_vec() {
    let temp_dir = TempDir::new().unwrap();
    let results: Vec<ScrapedContent> = vec![];

    let processed = export_urls(&results, &temp_dir, ExportFormat::Jsonl);

    assert!(processed.is_empty());
}

#[test]
fn hostile_export_filename_is_confined_inside_output_dir() {
    // Issue #1125 end-to-end containment through the public
    // `process_results` port. The output dir is nested one level deep
    // so any `../` escape would land inside the sandbox where the walk
    // can observe it — nothing touches the real filesystem outside.
    let sandbox = TempDir::new().unwrap();
    let output_dir = sandbox.path().join("lvl1").join("output");
    let results = vec![make_scraped_content(
        "https://example.com/page1",
        "Containment probe",
        "Content paragraph long enough to export through the pipeline.",
    )];

    for hostile in ["../escape", "sub/out", "..\\escape"] {
        process_results(
            &results,
            output_dir.clone(),
            ExportFormat::Jsonl,
            hostile,
            None,
        )
        .unwrap();
    }

    let mut escaped = Vec::new();
    for entry in walkdir::WalkDir::new(sandbox.path()).into_iter().flatten() {
        if entry.file_type().is_file() && !entry.path().starts_with(&output_dir) {
            escaped.push(entry.path().to_path_buf());
        }
    }
    assert!(escaped.is_empty(), "export escaped output_dir: {escaped:?}");
}

#[test]
fn test_process_results_single_item_exports_and_returns_url() {
    let temp_dir = TempDir::new().unwrap();
    let content = make_scraped_content(
        "https://example.com/page1",
        "Page One",
        "Content of page one",
    );

    let processed = export_urls(&[content], &temp_dir, ExportFormat::Jsonl);

    assert_eq!(processed.len(), 1);
    assert_eq!(processed[0], "https://example.com/page1");
    // Verify export file was created
    assert!(temp_dir.path().join("export.jsonl").exists());
}

#[test]
fn test_process_results_multiple_items_exports_all() {
    let temp_dir = TempDir::new().unwrap();
    let contents = vec![
        make_scraped_content("https://a.com/", "A", "Content A"),
        make_scraped_content("https://b.com/", "B", "Content B"),
        make_scraped_content("https://c.com/", "C", "Content C"),
    ];

    let processed = export_urls(&contents, &temp_dir, ExportFormat::Jsonl);

    assert_eq!(processed.len(), 3);
    // URLs get normalized through ValidUrl — check that all 3 are present
    assert_eq!(processed.len(), 3);
}

#[test]
fn process_results_dedupes_identical_url_and_content_within_run() {
    // #1215 (F-02): the exporter is append-only, so two identical items
    // in one run must yield ONE jsonl line — never two records that
    // differ only in `timestamp_utc`.
    let temp_dir = TempDir::new().unwrap();
    let first = make_scraped_content("https://dup.test/a", "Dup", "same body text here");
    let second = make_scraped_content("https://dup.test/a", "Dup", "same body text here");

    let processed = export_urls(&[first, second], &temp_dir, ExportFormat::Jsonl);

    assert_eq!(
        processed.len(),
        1,
        "the duplicate append must be skipped, not counted as processed"
    );
    let body = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
    assert_eq!(
        body.lines().filter(|l| !l.trim().is_empty()).count(),
        1,
        "one (url, content_hash) pair must produce exactly one line"
    );
}

#[test]
fn process_results_keeps_same_url_with_different_content() {
    // Same URL but a different body is a different (url, content_hash)
    // identity — both records must survive the in-run dedupe.
    let temp_dir = TempDir::new().unwrap();
    let first = make_scraped_content("https://dup.test/a", "Dup", "first body text here");
    let second = make_scraped_content("https://dup.test/a", "Dup", "second body text here");

    let processed = export_urls(&[first, second], &temp_dir, ExportFormat::Jsonl);

    assert_eq!(processed.len(), 2);
    let body = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
    assert_eq!(
        body.lines().filter(|l| !l.trim().is_empty()).count(),
        2,
        "distinct content hashes under one URL must both export"
    );
}

#[test]
fn process_results_keeps_same_content_under_different_urls() {
    // Mirrored content under distinct URLs is distinct records — the
    // dedupe key is (url, content_hash), never content alone.
    let temp_dir = TempDir::new().unwrap();
    let first = make_scraped_content("https://mirror.test/a", "A", "shared body text here");
    let second = make_scraped_content("https://mirror.test/b", "B", "shared body text here");

    let processed = export_urls(&[first, second], &temp_dir, ExportFormat::Jsonl);

    assert_eq!(processed.len(), 2);
    let body = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
    assert_eq!(
        body.lines().filter(|l| !l.trim().is_empty()).count(),
        2,
        "same content under different URLs must both export"
    );
}

#[test]
fn test_process_results_vector_format_creates_json_file() {
    let temp_dir = TempDir::new().unwrap();
    let content = make_scraped_content("https://example.com", "Test", "Body");

    let processed = export_urls(&[content], &temp_dir, ExportFormat::Vector);

    assert_eq!(processed.len(), 1);
    assert!(temp_dir.path().join("export.json").exists());
}

#[test]
fn test_process_results_invalid_content_records_failure_and_continues() {
    let temp_dir = TempDir::new().unwrap();
    // Empty content fails validation: per SC6 the item-level failure is
    // recorded (DomainRecoverable) and the run continues - it never aborts.
    let content = make_scraped_content("https://example.com", "Title", "");

    let result = process_results(
        &[content],
        temp_dir.path().to_path_buf(),
        ExportFormat::Jsonl,
        "export",
        None,
    );

    assert!(result.is_ok(), "item failures never abort the export");
}

// =========================================================================
// create_exporter tests
// =========================================================================
