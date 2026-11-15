//! `create_exporter` and `ExportFormat::Auto` branch tests (T2.3).

use super::tests_common::*;
use super::*;
use tempfile::TempDir;

#[test]
fn test_create_exporter_jsonl_returns_jsonl_exporter() {
    let temp_dir = TempDir::new().unwrap();
    let exporter = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Jsonl)
        .expect("Jsonl is a supported format");

    assert_eq!(exporter.format(), ExportFormat::Jsonl);
}

#[test]
fn test_create_exporter_vector_returns_vector_exporter() {
    let temp_dir = TempDir::new().unwrap();
    let exporter = create_exporter(temp_dir.path().to_path_buf(), "test", ExportFormat::Vector)
        .expect("Vector is a supported format");

    assert_eq!(exporter.format(), ExportFormat::Vector);
}

// =========================================================================
// ExportFormat::Auto branch tests (T2.3)
// =========================================================================

#[test]
fn test_auto_format_detects_jsonl_when_file_exists() {
    let temp_dir = TempDir::new().unwrap();
    // Create a .jsonl file to trigger detection
    std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

    let exporter = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto)
        .expect("auto detection must resolve");

    assert_eq!(exporter.format(), ExportFormat::Jsonl);
}

#[test]
fn test_auto_format_detects_vector_when_json_file_exists() {
    let temp_dir = TempDir::new().unwrap();
    // Create a .json file to trigger Vector detection. Only `.json` exists,
    // so the `.jsonl` probe in `create_auto_exporter` misses and the
    // `.json` branch is the one under test.
    std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

    let exporter = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto)
        .expect("auto detection must resolve");

    assert_eq!(exporter.format(), ExportFormat::Vector);
}

/// Both files present: `create_auto_exporter` probes `.jsonl` FIRST
/// (`if jsonl_path.exists() { .. } else if vector_path.exists() { .. }`),
/// so JSONL wins. The old name and comment claimed the opposite — pinned
/// here against the code as written, not against the stale prose.
#[test]
fn test_auto_format_jsonl_takes_priority_over_vector() {
    let temp_dir = TempDir::new().unwrap();
    std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();
    std::fs::write(temp_dir.path().join("export.json"), "").unwrap();

    let exporter = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto)
        .expect("auto detection must resolve");

    assert_eq!(
        exporter.format(),
        ExportFormat::Jsonl,
        "`.jsonl` is probed before `.json`, so it must win when both exist"
    );
}

#[test]
fn test_auto_format_falls_back_to_jsonl_when_no_files_exist() {
    let temp_dir = TempDir::new().unwrap();
    // No files exist — should default to Jsonl

    let exporter = create_exporter(temp_dir.path().to_path_buf(), "export", ExportFormat::Auto)
        .expect("auto detection must resolve");

    assert_eq!(exporter.format(), ExportFormat::Jsonl);
}

#[test]
fn test_auto_format_with_empty_dir_exports_successfully() {
    let temp_dir = TempDir::new().unwrap();
    let content = make_scraped_content("https://example.com", "Test", "Body");

    let processed = export_urls(&[content], &temp_dir, ExportFormat::Auto);

    assert_eq!(processed.len(), 1);
    // Default fallback creates .jsonl file
    assert!(temp_dir.path().join("export.jsonl").exists());
}

#[test]
fn test_auto_format_with_existing_jsonl_exports_to_jsonl() {
    let temp_dir = TempDir::new().unwrap();
    // Pre-create a .jsonl file
    std::fs::write(temp_dir.path().join("export.jsonl"), "").unwrap();

    let content = make_scraped_content("https://example.com", "Test", "Body");
    let processed = export_urls(&[content], &temp_dir, ExportFormat::Auto);

    assert_eq!(processed.len(), 1);
    // Should have appended to the existing .jsonl file
    let content = std::fs::read_to_string(temp_dir.path().join("export.jsonl")).unwrap();
    assert!(!content.is_empty());
}

// =========================================================================
// process_results with resume mode
// =========================================================================
