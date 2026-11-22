use super::*;

/// A real 64-char lowercase digest — the shape `ElasticIngestion`
/// produces. Short fakes like `"deadbeef"` used to pass as dedup keys
/// (#1118); they no longer do.
const HEX_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const HEX_B: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

fn sink(path: &str) -> StreamRepository {
    StreamRepository::new(SinkPath::parse(path).expect("valid sink path")).expect("open stream")
}

#[tokio::test]
async fn test_save_chunk_omits_record_without_embedding() {
    // Write to a temp file so we can assert no line is produced.
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = sink(&path);

    repo.save_resource("https://example.com", "", HEX_A, 10)
        .await
        .expect("save_resource");
    // No embedding → record omitted, write must succeed without a line.
    repo.save_chunk(
        &format!("{HEX_A}-0"),
        "https://example.com",
        0,
        "hello",
        None,
        None,
    )
    .await
    .expect("save_chunk");

    let contents = std::fs::read_to_string(&path).expect("read stream");
    assert!(
        contents.trim().is_empty(),
        "no line expected when embedding is None"
    );
}

#[tokio::test]
async fn test_save_chunk_emits_384_dim_embedding_and_hash() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = sink(&path);

    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
    repo.save_resource("https://example.com/p", "Page Title", HEX_A, 42)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &format!("{HEX_A}-0"),
        "https://example.com/p",
        0,
        "cleaned chunk text",
        Some(&embedding),
        None,
    )
    .await
    .expect("save_chunk");

    let contents = std::fs::read_to_string(&path).expect("read stream");
    let line = contents.lines().next().expect("one JSONL line");
    let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

    assert_eq!(record.sha256_hex.as_str(), HEX_A);
    assert_eq!(record.embedding.len(), 384, "explicit 384-dim embedding");
    assert_eq!(record.title.as_deref(), Some("Page Title"));
    assert_eq!(record.chunk_text, "cleaned chunk text");
}

// Collecting writer that buffers everything in memory so tests can inspect
// the exact JSONL bytes that `StreamRepository` emits.
/// D2 — a broken-pipe write error must surface as `Err(ScraperError::Io)`,
/// never as a panic. Uses a deterministic in-memory writer stub (no OS pipes).
#[tokio::test]
async fn contract_broken_pipe_returns_err_not_panic() {
    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "simulated broken pipe",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let repo = StreamRepository::from_writer(Box::new(BrokenPipeWriter));
    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();

    let result = repo
        .save_chunk(
            &format!("{HEX_B}-0"),
            "https://example.com/p",
            0,
            "cleaned chunk text",
            Some(&embedding),
            None,
        )
        .await;

    assert!(
        result.is_err(),
        "broken pipe must return Err, not panic/panic"
    );
    let err = result.expect_err("broken pipe error");
    assert!(
        matches!(err, ScraperError::Io(_)),
        "broken pipe must map to ScraperError::Io, got: {err:?}"
    );
}

/// #1119 — a poisoned writer lock must surface as a typed `ScraperError::Io`
/// from `save_chunk`, never as a panic inside the sink future (the old
/// `.expect("vector stream poisoned")` aborted the ingestion task).
#[tokio::test]
async fn contract_poisoned_writer_lock_returns_err_not_panic() {
    struct NullWriter;
    impl Write for NullWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let repo = StreamRepository::from_writer(Box::new(NullWriter));
    // Poison the writer mutex: panic while holding the lock on a separate
    // thread, then wait for the panic to land.
    let writer = Arc::clone(&repo.writer);
    let poisoner = std::thread::spawn(move || {
        // `unwrap` is test-only: the lock is fresh and cannot fail here.
        let _guard = writer.lock().unwrap();
        panic!("poison the vector stream lock");
    });
    assert!(poisoner.join().is_err(), "poisoner must have panicked");

    // #1118: the id and URL must pass the typed-boundary validation so the
    // call actually reaches the poisoned writer lock.
    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
    let result = repo
        .save_chunk(
            &format!("{HEX_B}-0"),
            "https://example.com/p",
            0,
            "cleaned chunk text",
            Some(&embedding),
            None,
        )
        .await;

    assert!(
        result.is_err(),
        "poisoned writer lock must return Err, not panic"
    );
    let err = result.expect_err("poison error");
    assert!(
        matches!(err, ScraperError::Io(_)),
        "poison must map to ScraperError::Io, got: {err:?}"
    );
}

/// 384-dim embedding integrity: round-trips exactly (same values + length)
/// and the raw JSON line carries a 384-length array.
#[tokio::test]
async fn contract_embedding_384_dim_roundtrip() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = sink(&path);

    let embedding: Vec<f32> = (0..384).map(|i| i as f32 * 0.001).collect();
    repo.save_resource("https://example.com/p", "Title", HEX_B, 42)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &format!("{HEX_B}-0"),
        "https://example.com/p",
        0,
        "cleaned chunk text",
        Some(&embedding),
        None,
    )
    .await
    .expect("save_chunk");

    let contents = std::fs::read_to_string(&path).expect("read stream");
    let line = contents.lines().next().expect("one JSONL line");
    let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

    assert_eq!(record.embedding.len(), 384, "embedding must be 384 floats");
    assert_eq!(
        record.embedding, embedding,
        "deserialized embedding must equal the original values"
    );

    // Raw JSON line carries a 384-length array in the "embedding" field.
    let value: serde_json::Value = serde_json::from_str(line).expect("valid JSON");
    let arr = value
        .get("embedding")
        .expect("embedding field present")
        .as_array()
        .expect("embedding is a JSON array");
    assert_eq!(
        arr.len(),
        384,
        "raw JSON embedding array must be length 384"
    );
}

/// Lowercase-hex SHA-256 integrity: what upstream provides (a real 64-char
/// lowercase digest) is written verbatim and stays lowercase.
#[tokio::test]
async fn contract_sha256_hex_is_lowercase_and_preserved() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = sink(&path);

    let id = format!("{HEX_A}-0");
    repo.save_resource("https://example.com/p", "T", HEX_A, 1)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &id,
        "https://example.com/p",
        0,
        "cleaned chunk text",
        Some(&vec![0.0f32; 384]),
        None,
    )
    .await
    .expect("save_chunk");

    let contents = std::fs::read_to_string(&path).expect("read stream");
    let line = contents.lines().next().expect("one JSONL line");
    let record: VectorRecord = serde_json::from_str(line).expect("valid JSONL");

    assert_eq!(
        record.sha256_hex.as_str(),
        HEX_A,
        "sha256_hex must be preserved verbatim from the id"
    );

    let is_lowercase_hex = record
        .sha256_hex
        .as_str()
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        && record.sha256_hex.as_str().len() == Sha256Hex::HEX_LEN;
    assert!(
        is_lowercase_hex,
        "sha256_hex must be 64 lowercase hex chars (no uppercase), got: {}",
        record.sha256_hex
    );
}

/// #1118 reproduction: the sink used to write ANY string carried by the
/// chunk id as the dedup key — a 32-char fake like `"stubhash-0"` (the
/// old test fixture shape) produced a valid-looking JSONL line. Now the
/// malformed key and a non-fetchable URL are rejected at the boundary
/// and no corrupt line reaches the stream.
#[tokio::test]
async fn issue_1118_sink_rejects_malformed_dedup_key_and_url() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = sink(&path);
    let embedding: Vec<f32> = vec![0.0f32; 384];

    let err = repo
        .save_chunk(
            "stubhash-0",
            "https://example.com/p",
            0,
            "x",
            Some(&embedding),
            None,
        )
        .await
        .expect_err("32-char fake hash must be rejected at the sink");
    assert!(
        err.to_string().to_lowercase().contains("sha256"),
        "rejection must name the hash, got: {err}"
    );

    let err = repo
        .save_chunk(
            &format!("{HEX_A}-0"),
            "data:text/html,x",
            0,
            "x",
            Some(&embedding),
            None,
        )
        .await
        .expect_err("data: URL must be rejected at the sink");
    assert!(
        err.to_string().contains("no soportado"),
        "rejection must name the scheme, got: {err}"
    );

    let contents = std::fs::read_to_string(&path).expect("read stream");
    assert!(
        contents.trim().is_empty(),
        "rejected records must not reach the stream"
    );
}

/// #1118: the `"-"` stdout sentinel is a variant now; an empty path is
/// rejected at the boundary instead of reaching `File::create("")`.
#[test]
fn issue_1118_sink_path_is_validated() {
    assert_eq!("-".parse::<SinkPath>().expect("sentinel"), SinkPath::Stdout);
    assert!(
        SinkPath::parse("").is_err(),
        "empty sink path must be rejected at the boundary"
    );
    assert!(
        SinkPath::parse("   ").is_err(),
        "whitespace-only sink path must be rejected"
    );
    assert!(matches!(
        SinkPath::parse("out/vectors.jsonl"),
        Ok(SinkPath::File(_))
    ));
}

// ---- #2429: embedding provenance and chunk position -------------------

/// Sink over a temp file carrying fixed provenance (3-dim mock model).
fn provenance_sink(path: &str) -> StreamRepository {
    StreamRepository::from_writer_with_provenance(
        Box::new(std::fs::File::create(path).expect("temp file")),
        Some(EmbeddingProvenance {
            model_id: "granite-97m".to_string(),
            embedding_dim: 3,
        }),
    )
}

/// Read back the single JSONL record written to `path`.
fn last_record(path: &str) -> VectorRecord {
    let contents = std::fs::read_to_string(path).expect("read stream");
    let line = contents.lines().next().expect("one JSONL line");
    VectorRecord::from_line(line).expect("valid JSONL")
}

/// #2429: every emitted record must say which model produced its vector,
/// at which dimension, and where the chunk sat in the document — the
/// identity that makes vectors comparable across a result set.
#[tokio::test]
async fn record_carries_embedding_provenance_and_chunk_index() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = provenance_sink(&path);

    repo.save_resource("https://example.com/p", "Page", HEX_A, 10)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &format!("{HEX_A}-7"),
        "https://example.com/p",
        7,
        "chunk body",
        Some(&[0.1, 0.2, 0.3]),
        None,
    )
    .await
    .expect("save_chunk");

    let record = last_record(&path);
    assert_eq!(record.embedding_model.as_deref(), Some("granite-97m"));
    assert_eq!(record.embedding_dim, Some(3));
    assert_eq!(record.chunk_index, Some(7));
}

/// #2429: a vector whose length disagrees with the declared model
/// dimension is silently unusable for similarity search — the sink must
/// reject it instead of writing an incomparable record.
#[tokio::test]
async fn sink_rejects_embedding_dim_mismatch() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = provenance_sink(&path);

    let result = repo
        .save_chunk(
            &format!("{HEX_A}-0"),
            "https://example.com/p",
            0,
            "chunk body",
            Some(&[0.1, 0.2, 0.3, 0.4]),
            None,
        )
        .await;

    assert!(result.is_err(), "dim mismatch must be rejected at the sink");
}

/// #2429: the read boundary rejects a record whose vector length
/// contradicts its declared `embedding_dim`.
#[test]
fn record_from_line_rejects_dim_mismatch() {
    let line = format!(
        r#"{{"url":"https://example.com","sha256_hex":"{HEX_A}","title":null,"chunk_text":"x","embedding":[0.1,0.2],"embedding_dim":3,"timestamp":"2026-01-01T00:00:00Z"}}"#
    );
    let err = VectorRecord::from_line(&line).expect_err("dim mismatch must be rejected");
    assert!(matches!(err, ScraperError::Ingestion(_)), "got: {err:?}");
}

/// #2429: schema compatibility — records written before provenance
/// existed still deserialize; the new fields read as `None`.
#[test]
fn record_from_line_accepts_legacy_line_without_new_fields() {
    let line = format!(
        r#"{{"url":"https://example.com","sha256_hex":"{HEX_A}","title":null,"chunk_text":"x","embedding":[0.1],"timestamp":"2026-01-01T00:00:00Z"}}"#
    );
    let record = VectorRecord::from_line(&line).expect("legacy line must parse");
    assert_eq!(record.embedding_model, None);
    assert_eq!(record.embedding_dim, None);
    assert_eq!(record.chunk_index, None);
}

// ---- #2429 slice 2: chunk byte range in the source document -----------

/// #2429 AC2: a record must say where the chunk sat in the source
/// document — the enclosing byte range of the chunk's spans.
#[tokio::test]
async fn record_carries_chunk_byte_range() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = provenance_sink(&path);

    let spans = vec![
        crate::domain::text_chunker::ByteSpan::try_new(100, 150).expect("valid span"),
        crate::domain::text_chunker::ByteSpan::try_new(160, 220).expect("valid span"),
    ];
    repo.save_resource("https://example.com/p", "Page", HEX_A, 10)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &format!("{HEX_A}-0"),
        "https://example.com/p",
        0,
        "chunk body",
        Some(&[0.1, 0.2, 0.3]),
        Some(&spans),
    )
    .await
    .expect("save_chunk");

    let record = last_record(&path);
    assert_eq!(record.byte_start, Some(100), "first span start");
    assert_eq!(record.byte_end, Some(220), "last span end");
}

/// #2429 AC2: a chunk with no locator information emits the range as
/// `null` — absence is explicit, never a fabricated 0.
#[tokio::test]
async fn record_without_locator_emits_null_range() {
    let tmp = tempfile::NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_string_lossy().to_string();
    let repo = provenance_sink(&path);

    repo.save_resource("https://example.com/p", "Page", HEX_A, 10)
        .await
        .expect("save_resource");
    repo.save_chunk(
        &format!("{HEX_A}-0"),
        "https://example.com/p",
        0,
        "chunk body",
        Some(&[0.1, 0.2, 0.3]),
        None,
    )
    .await
    .expect("save_chunk");

    let record = last_record(&path);
    assert_eq!(record.byte_start, None);
    assert_eq!(record.byte_end, None);
}

/// #2429 AC2: the read boundary rejects a record whose byte range is
/// inverted — it cannot name a real region of any document.
#[test]
fn record_from_line_rejects_inverted_byte_range() {
    let line = format!(
        r#"{{"url":"https://example.com","sha256_hex":"{HEX_A}","title":null,"chunk_text":"x","embedding":[0.1],"byte_start":30,"byte_end":10,"timestamp":"2026-01-01T00:00:00Z"}}"#
    );
    let err = VectorRecord::from_line(&line).expect_err("inverted range must be rejected");
    assert!(matches!(err, ScraperError::Ingestion(_)), "got: {err:?}");
}
