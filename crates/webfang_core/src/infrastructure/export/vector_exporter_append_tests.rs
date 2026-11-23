use std::path::PathBuf;

use super::*;
use crate::domain::config::ExportFormat;

fn create_test_config_with_dir(dir: PathBuf) -> ExporterConfig {
    ExporterConfig::new(dir, ExportFormat::Vector, "test_export")
}

fn create_test_chunk() -> DocumentChunkValidated {
    use crate::domain::Draft;
    // Create DocumentChunk via From<ScrapedContent> then validate
    let scraped = crate::domain::ScrapedContent {
        title: "Test Document".to_string(),
        content: "Test content for vector export".to_string(),
        url: crate::domain::ValidUrl::parse("https://example.com/test").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![],
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = crate::domain::DocumentChunk::<Draft>::from(scraped);
    chunk.validate().unwrap()
}

// --- Task 4.4: Append mode test ---

#[test]
fn test_vector_exporter_append_mode_preserves_documents() {
    let temp_dir = std::env::temp_dir().join("test_vector_append");
    let _ = std::fs::remove_dir_all(&temp_dir);

    // First batch: write 2 documents without append
    let mut config1 = create_test_config_with_dir(temp_dir.clone());
    config1.append = false;
    let exporter1 = VectorExporter::new(config1);

    let docs1 = vec![create_test_chunk(), create_test_chunk()];
    let result = exporter1.export_batch(&docs1);
    assert!(
        result.is_ok(),
        "first batch should succeed: {:?}",
        result.err()
    );

    let file1_path = temp_dir.join("test_export.json");
    assert!(
        file1_path.exists(),
        "output file should exist after first batch"
    );

    // Read file content after first write
    let content1 = std::fs::read_to_string(&file1_path).expect("should read file");
    let json1: serde_json::Value =
        serde_json::from_str(&content1).expect("first write should produce valid JSON");
    let first_doc_count = json1["documents"].as_array().map_or(0, |a| a.len());
    assert_eq!(first_doc_count, 2, "first batch should have 2 documents");

    // Second batch: append 1 document with append=true
    let mut config2 = create_test_config_with_dir(temp_dir.clone());
    config2.append = true;
    let exporter2 = VectorExporter::new(config2);

    let doc3 = create_test_chunk();
    let result = exporter2.export(doc3);
    assert!(result.is_ok(), "append should succeed: {:?}", result.err());

    // Read final file and verify all 3 documents are present
    let content2 = std::fs::read_to_string(&file1_path).expect("should read file after append");
    let json2: serde_json::Value =
        serde_json::from_str(&content2).expect("after append should be valid JSON");
    let final_doc_count = json2["documents"].as_array().map_or(0, |a| a.len());
    assert_eq!(
        final_doc_count, 3,
        "should have 3 documents after append (2 + 1)"
    );

    // Verify metadata header still exists
    assert!(
        json2.get("format_version").is_some() || json2.get("metadata").is_some(),
        "metadata header should still be present after append"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// --- Task 4.9: Directory creation failure test ---

/// An output base directory that is GUARANTEED unusable, per OS:
///
/// - unix: `/root/no-permission/…` — the superuser home is not writable
///   by the test user, so `create_dir_all` fails with EACCES.
/// - windows: `C:\inva:lid\…` — a colon in a path segment other than
///   the drive prefix is an invalid NTFS path (ERROR_INVALID_NAME), so
///   directory creation fails for every user, permission-free. `/root`
///   would be a perfectly valid (and usually creatable) path on
///   Windows, so the unix fixture cannot be reused there.
#[cfg(unix)]
#[test]
fn test_vector_exporter_directory_creation_fails() {
    let config = ExporterConfig::new(
        PathBuf::from("/root/no-permission/test_vector"),
        ExportFormat::Vector,
        "test_export",
    );
    let exporter = VectorExporter::new(config);
    let doc = create_test_chunk();

    let result = exporter.export(doc);
    assert!(
        result.is_err(),
        "export to /root should fail with directory creation error"
    );
}

/// Windows counterpart of the directory-creation-failure pin: an
/// invalid-character path (`:` mid-segment) fails for every user,
/// permission-free. COMPILE-PROOF ONLY from the Linux checkout — the
/// Windows advisory lane must verify it at runtime (#1608).
#[cfg(windows)]
#[test]
fn test_vector_exporter_directory_creation_fails() {
    let config = ExporterConfig::new(
        PathBuf::from(r"C:\inva:lid\no-such-dir\test_vector"),
        ExportFormat::Vector,
        "test_export",
    );
    let exporter = VectorExporter::new(config);
    let doc = create_test_chunk();

    let result = exporter.export(doc);
    assert!(
        result.is_err(),
        "export to a path with an invalid character should fail with directory creation error"
    );
}

// --- Task 4.10: Serialization failure with NaN in embeddings ---

#[test]
fn test_vector_exporter_serialization_nan_fails() {
    let temp_dir = std::env::temp_dir().join("test_vector_nan");
    let _ = std::fs::remove_dir_all(&temp_dir);

    let config = create_test_config_with_dir(temp_dir.clone());
    let exporter = VectorExporter::new(config);

    // Create a document with NaN in embeddings — serde_json rejects NaN by default
    let mut doc = create_test_chunk();
    doc.embeddings = Some(vec![0.1, f32::NAN, 0.3]);

    let result = exporter.export(doc);
    assert!(
        result.is_err(),
        "export with NaN in embeddings should fail with serialization error"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Error path tests
// ============================================================================

#[test]
fn test_export_batch_vs_individual_consistency() {
    let temp_dir = std::env::temp_dir().join("test_batch_vs_individual");
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Export as batch
    let config_batch = create_test_config_with_dir(temp_dir.join("batch"));
    let exporter_batch = VectorExporter::new(config_batch);
    let chunks = vec![create_test_chunk(), create_test_chunk()];
    exporter_batch.export_batch(&chunks).unwrap();

    // Export individually with append mode
    let mut config_ind = create_test_config_with_dir(temp_dir.join("individual"));
    config_ind.append = true;
    let exporter_ind = VectorExporter::new(config_ind);
    let chunk1 = create_test_chunk();
    let chunk2 = create_test_chunk();
    exporter_ind.export(chunk1).unwrap();
    exporter_ind.export(chunk2).unwrap();

    // Both should produce valid JSON
    let individual_path = temp_dir.join("individual/test_export.json");
    let batch_path = temp_dir.join("batch/test_export.json");

    let individual_content = std::fs::read_to_string(&individual_path).unwrap();
    let batch_content = std::fs::read_to_string(&batch_path).unwrap();

    let individual_json: serde_json::Value = serde_json::from_str(&individual_content).unwrap();
    let batch_json: serde_json::Value = serde_json::from_str(&batch_content).unwrap();

    // Both should have 2 documents
    let individual_docs = individual_json["documents"].as_array().unwrap();
    let batch_docs = batch_json["documents"].as_array().unwrap();
    assert!(!individual_docs.is_empty());
    assert!(!batch_docs.is_empty());
    assert_eq!(individual_docs.len(), 2);
    assert_eq!(batch_docs.len(), 2);

    // Both should have the same metadata structure
    assert!(individual_json.get("format_version").is_some());
    assert!(batch_json.get("format_version").is_some());
    assert_eq!(
        individual_json["format_version"],
        batch_json["format_version"]
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

#[test]
fn test_export_append_to_existing_file() {
    let temp_dir = std::env::temp_dir().join("test_append_existing");
    let _ = std::fs::remove_dir_all(&temp_dir);

    // First write without append
    let mut config1 = create_test_config_with_dir(temp_dir.clone());
    config1.append = false;
    let exporter1 = VectorExporter::new(config1);
    let first_chunk = create_test_chunk();
    exporter1.export(first_chunk).unwrap();

    // Read initial content
    let output_path = temp_dir.join("test_export.json");
    let initial_content = std::fs::read_to_string(&output_path).unwrap();
    let initial_json: serde_json::Value = serde_json::from_str(&initial_content).unwrap();
    let docs = initial_json["documents"].as_array().unwrap();
    assert!(!docs.is_empty());
    assert_eq!(docs.len(), 1);

    // Second write with append
    let mut config2 = create_test_config_with_dir(temp_dir.clone());
    config2.append = true;
    let exporter2 = VectorExporter::new(config2);
    let second_chunk = create_test_chunk();
    exporter2.export(second_chunk).unwrap();

    // Read final content
    let final_content = std::fs::read_to_string(&output_path).unwrap();
    let final_json: serde_json::Value = serde_json::from_str(&final_content).unwrap();
    assert_eq!(
        final_json["documents"].as_array().unwrap().len(),
        2,
        "should have 2 docs after append"
    );

    let _ = std::fs::remove_dir_all(&temp_dir);
}

// ============================================================================
// Issue #502: header total_documents must match the documents array
// ============================================================================

/// Parse `<dir>/test_export.json` as JSON.
fn read_export_json(dir: &std::path::Path) -> serde_json::Value {
    let path = dir.join("test_export.json");
    let content = std::fs::read_to_string(&path).expect("export file should exist");
    serde_json::from_str(&content).expect("export must be valid JSON")
}

/// ExporterConfig with append=true — the exact production shape
/// (`create_exporter` builds every exporter with `.with_append(true)`).
fn append_config(dir: PathBuf) -> ExporterConfig {
    create_test_config_with_dir(dir).with_append(true)
}

/// Issue #502 repro: production always uses append mode, so a fresh
/// export must still report the real document count in the header.
#[test]
fn test_fresh_file_in_append_mode_reports_total_documents() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
    let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));

    let docs = vec![
        create_test_chunk(),
        create_test_chunk(),
        create_test_chunk(),
    ];
    exporter
        .export_batch(&docs)
        .expect("batch export should succeed");

    let json = read_export_json(temp_dir.path());
    let documents = json["documents"]
        .as_array()
        .expect("documents must be an array");
    assert_eq!(documents.len(), 3, "all 3 documents must be present");
    assert_eq!(
        json["total_documents"].as_u64(),
        Some(3),
        "total_documents must equal the documents array length"
    );
}

#[test]
fn test_cross_run_appends_accumulate_total_documents() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
    let config = append_config(temp_dir.path().to_path_buf());

    let first_run = VectorExporter::new(config.clone());
    first_run
        .export_batch(&[create_test_chunk(), create_test_chunk()])
        .expect("first batch should succeed");

    let second_run = VectorExporter::new(config);
    second_run
        .export_batch(&[create_test_chunk(), create_test_chunk()])
        .expect("second batch should succeed");

    let json = read_export_json(temp_dir.path());
    assert_eq!(
        json["total_documents"].as_u64(),
        Some(4),
        "total must accumulate across runs"
    );
    assert_eq!(json["documents"].as_array().map(Vec::len), Some(4));
}

#[test]
fn test_append_single_export_after_batch_accumulates_total() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
    let config = append_config(temp_dir.path().to_path_buf());

    let first_run = VectorExporter::new(config.clone());
    first_run
        .export_batch(&[create_test_chunk(), create_test_chunk()])
        .expect("batch should succeed");

    let second_run = VectorExporter::new(config);
    second_run
        .export(create_test_chunk())
        .expect("single export should succeed");

    let json = read_export_json(temp_dir.path());
    assert_eq!(
        json["total_documents"].as_u64(),
        Some(3),
        "total must accumulate across runs"
    );
    assert_eq!(json["documents"].as_array().map(Vec::len), Some(3));
}

#[test]
fn test_legacy_header_is_migrated_on_append() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
    let path = temp_dir.path().join("test_export.json");

    let doc1 = serde_json::to_string(&create_test_chunk()).expect("serialize doc1");
    let doc2 = serde_json::to_string(&create_test_chunk()).expect("serialize doc2");
    // Exact shape written by the pre-fix exporter: hardcoded count, no
    // reserved windows, one line per document, closing bracket.
    let legacy = format!(
        "{{\"format_version\": \"1.0\", \"model_name\": null, \"dimensions\": null, \
         \"total_documents\": 0, \"created_at\": \"2026-01-01T00:00:00+00:00\", \
         \"documents\": [{doc1}\n,{doc2}\n]}}\n"
    );
    std::fs::write(&path, legacy).expect("legacy file should be written");

    let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));
    exporter
        .export(create_test_chunk())
        .expect("append to legacy file should succeed");

    let content = std::fs::read_to_string(&path).expect("read migrated file");
    let json: serde_json::Value =
        serde_json::from_str(&content).expect("migrated file must be valid JSON");
    assert_eq!(
        json["total_documents"].as_u64(),
        Some(3),
        "migrated header must report the real document count"
    );
    assert_eq!(json["documents"].as_array().map(Vec::len), Some(3));
    assert_eq!(
        json["created_at"].as_str(),
        Some("2026-01-01T00:00:00+00:00"),
        "migration must preserve the original created_at"
    );
}

#[test]
fn test_dimensions_are_learned_and_written_to_header() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir should be created");
    let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));

    let mut doc = create_test_chunk();
    doc.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]);
    exporter
        .export_batch(&[doc])
        .expect("batch with embeddings should succeed");

    let json = read_export_json(temp_dir.path());
    assert_eq!(
        json["dimensions"].as_u64(),
        Some(4),
        "header dimensions must reflect the learned embedding size"
    );
}

#[test]
fn append_refuses_to_corrupt_a_file_with_a_torn_document() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let path = temp_dir.path().join("test_export.json");
    let original = "{\"format_version\": \"1.0\", \"total_documents\": 1, \"created_at\": \"2026-01-01T00:00:00+00:00\", \"documents\": [{\"id\":\"torn\"";
    std::fs::write(&path, original).expect("write torn fixture");

    let exporter = VectorExporter::new(append_config(temp_dir.path().to_path_buf()));
    let err = exporter
        .export(create_test_chunk())
        .expect_err("append must fail instead of closing a torn document");

    assert!(
        format!("{err}").contains("no closing bracket"),
        "unexpected error: {err}"
    );
    assert_eq!(
        std::fs::read(&path).expect("read torn fixture"),
        original.as_bytes(),
        "a refused append must leave the file byte-identical"
    );
}
