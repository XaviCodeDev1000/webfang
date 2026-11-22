use std::path::PathBuf;

use super::super::file_exporter::capture_export_trace;
use super::*;
use crate::domain::config::ExportFormat;

/// #1610 (OBS-H6): the vector exporter serializes each document itself, so
/// its span reports the exact per-document byte volume it appended (the
/// header and closing bracket are written by the exporter around the
/// documents, and are deliberately excluded — the key documents ONE
/// figure, the same one the file/jsonl exporters spell).
#[test]
fn export_batch_span_records_outcome_and_payload_bytes() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let exporter = VectorExporter::new(create_test_config_with_dir(temp_dir.path().to_path_buf()));
    let chunks = vec![create_test_chunk(), create_test_chunk()];
    let expected: u64 = chunks
        .iter()
        .map(|c| serde_json::to_string(c).expect("serialize").len() as u64)
        .sum();

    let records = capture_export_trace(|| async {
        exporter
            .export_batch(&chunks)
            .expect("batch export must succeed");
    });

    let closes: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == "export_batch")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one export_batch span");
    let fields = closes[0]["span_fields"].as_object().expect("span_fields");
    assert_eq!(fields.get("outcome").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        fields.get("exporter").and_then(|v| v.as_str()),
        Some("vector")
    );
    assert_eq!(fields.get("documents").and_then(|v| v.as_u64()), Some(2));
    assert_eq!(
        fields.get("payload_bytes").and_then(|v| v.as_u64()),
        Some(expected),
        "the recorded payload must be the serialized document bytes"
    );
}

/// #1610: an empty batch is a SUCCESS with a zero volume — the token must
/// not be inferred from the byte count.
#[test]
fn export_batch_span_records_ok_for_empty_batch() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    let exporter = VectorExporter::new(create_test_config_with_dir(temp_dir.path().to_path_buf()));

    let records = capture_export_trace(|| async {
        exporter.export_batch(&[]).expect("empty batch is a no-op");
    });

    let closes: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == "export_batch")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one export_batch span");
    let fields = closes[0]["span_fields"].as_object().expect("span_fields");
    assert_eq!(fields.get("outcome").and_then(|v| v.as_str()), Some("ok"));
    assert_eq!(
        fields.get("payload_bytes").and_then(|v| v.as_u64()),
        Some(0)
    );
}

/// #1610: the failure arm — a batch that cannot open its output file must
/// close the span as `error`, with no payload claim attached.
#[test]
fn export_batch_span_records_error_outcome() {
    let temp_dir = tempfile::TempDir::new().expect("tempdir");
    // A plain FILE where the output directory is required: `create_dir_all`
    // fails for every user on every OS, with no permission fixture.
    let blocked = temp_dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").expect("fixture file");
    let exporter = VectorExporter::new(create_test_config_with_dir(blocked));

    let records = capture_export_trace(|| async {
        let result = exporter.export_batch(&[create_test_chunk()]);
        assert!(result.is_err(), "unwritable output must fail the batch");
    });

    let closes: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == "export_batch")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one export_batch span");
    let fields = closes[0]["span_fields"].as_object().expect("span_fields");
    assert_eq!(
        fields.get("outcome").and_then(|v| v.as_str()),
        Some("error")
    );
    assert!(
        !fields.contains_key("payload_bytes"),
        "a failed batch wrote no payload — the key must be absent: {fields:?}"
    );
}

fn create_test_config_with_dir(dir: PathBuf) -> ExporterConfig {
    ExporterConfig::new(dir, ExportFormat::Vector, "test_export")
}

fn create_test_config() -> ExporterConfig {
    ExporterConfig::new(
        PathBuf::from("/tmp/test_vector_export"),
        ExportFormat::Vector,
        "test_export",
    )
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

#[test]
fn test_cosine_similarity_identical() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![1.0, 0.0, 0.0];
    let result = cosine_similarity(&a, &b).unwrap();
    assert!((result - 1.0).abs() < 1e-6);
}

#[test]
fn test_cosine_similarity_orthogonal() {
    let a = vec![1.0, 0.0, 0.0];
    let b = vec![0.0, 1.0, 0.0];
    let result = cosine_similarity(&a, &b).unwrap();
    assert!(result.abs() < 1e-6);
}

#[test]
fn test_cosine_similarity_zero_magnitude() {
    let a = vec![0.0, 0.0, 0.0];
    let b = vec![1.0, 2.0, 3.0];
    let result = cosine_similarity(&a, &b).unwrap();
    assert_eq!(result, 0.0);
}

#[test]
fn test_cosine_similarity_normal() {
    let a = vec![1.0, 2.0, 3.0];
    let b = vec![4.0, 5.0, 6.0];
    let result = cosine_similarity(&a, &b).unwrap();
    // Expected: (1*4 + 2*5 + 3*6) / (sqrt(14) * sqrt(77))
    // = 32 / (3.741... * 8.774...) ≈ 0.9746
    assert!((result - 0.9746).abs() < 1e-3);
}

#[test]
fn test_cosine_similarity_dimension_mismatch() {
    let a = vec![1.0, 2.0];
    let b = vec![1.0, 2.0, 3.0];
    let result = cosine_similarity(&a, &b);
    // The variant carries BOTH lengths, so it pins which side is reported
    // as `expected` (the reference, `b`) and which as `actual` (the
    // argument, `a`) — a bare `is_err()` pinned neither.
    assert!(
        matches!(
            result,
            Err(ExporterError::DimensionMismatch {
                expected: 3,
                actual: 2
            })
        ),
        "mismatched lengths must report DimensionMismatch {{ expected: 3, actual: 2 }}"
    );
}

#[test]
fn test_vector_exporter_creation() {
    let config = create_test_config();
    let exporter = VectorExporter::new(config.clone());
    assert_eq!(exporter.config().output_dir, config.output_dir);
    assert_eq!(exporter.config().format, ExportFormat::Vector);
}

#[test]
fn test_vector_exporter_with_path() {
    let config = create_test_config();
    let custom_path = PathBuf::from("/custom/path");
    let exporter = VectorExporter::new_with_path(config, custom_path.clone());
    assert_eq!(exporter.config().output_dir, custom_path);
}

#[test]
fn test_serialize_document_with_embeddings() {
    let config = create_test_config();
    let exporter = VectorExporter::new(config);

    // Create document and manually add embeddings
    let mut doc = create_test_chunk();
    doc.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]); // Add embeddings

    let result = exporter.serialize_document(&doc);
    assert!(result.is_ok());

    let json_str = result.unwrap();
    // embeddings field present because we added it
    assert!(
        json_str.contains("embeddings"),
        "expected embeddings field when embeddings is Some"
    );
    assert!(json_str.contains("Test Document"));
}

#[test]
fn test_serialize_document_dimension_mismatch() {
    let config = create_test_config();
    let exporter = VectorExporter::new(config);

    // First document sets dimensions
    let mut doc1 = create_test_chunk();
    doc1.embeddings = Some(vec![0.1, 0.2, 0.3, 0.4]); // 4 dimensions
    let _ = exporter.serialize_document(&doc1);

    // Second document with different dimensions - should warn and serialize without embeddings
    let mut doc2 = create_test_chunk();
    doc2.embeddings = Some(vec![0.1, 0.2]); // Only 2 dimensions

    let result = exporter.serialize_document(&doc2);
    assert!(
        result.is_ok(),
        "dimension mismatch should serialize without embeddings, got: {result:?}"
    );

    let json_str = result.unwrap();
    // Should serialize without embeddings (not an error)
    assert!(
        !json_str.contains("\"embeddings\""),
        "embeddings should be null/absent in output when dimension mismatch"
    );
}

#[test]
fn test_serialize_document_without_embeddings() {
    let config = create_test_config();
    let exporter = VectorExporter::new(config);

    let mut doc = create_test_chunk();
    doc.embeddings = None;

    let result = exporter.serialize_document(&doc);
    assert!(result.is_ok());

    let json_str = result.unwrap();
    // embeddings field is skipped when None (skip_serializing_if)
    assert!(!json_str.contains("embeddings"));
    assert!(json_str.contains("Test Document"));
}

#[test]
fn test_export_batch_empty() {
    let config = create_test_config();
    let exporter = VectorExporter::new(config);

    let result = exporter.export_batch(&[]);
    assert!(result.is_ok());
}

// Shared fixtures are mirrored in the sibling append/header test module.
