use super::*;
use crate::domain::error::CrawlError;

#[test]
fn test_invalid_url_error() {
    let err = ScraperError::invalid_url("URL vacía");
    assert_eq!(err.to_string(), "URL inválida: URL vacía");
}

#[test]
fn test_http_error() {
    let err = ScraperError::http(404, "https://example.com");
    assert!(
        matches!(err, ScraperError::Http { status: 404, .. }),
        "HTTP variant with status 404 must be preserved"
    );
}

#[test]
fn test_readability_error() {
    let err = ScraperError::readability("Failed to parse HTML");
    assert_eq!(
        err.to_string(),
        "Error de legibilidad: Failed to parse HTML"
    );
}

#[test]
fn test_io_error_from_std() {
    let std_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let err: ScraperError = std_err.into();
    assert!(
        matches!(err, ScraperError::Io(_)),
        "std::io::Error must convert to ScraperError::Io"
    );
}

// Matrix rows 21/22: `ScraperError::Io` must classify by
// `io::ErrorKind`, mirroring `CrawlError::classify` — transient kinds
// are retriable, permanent kinds (NotFound, PermissionDenied, rest)
// are fatal-permanent, never InternalFatal.
#[test]
fn classify_io_transient_kind_is_transient_retriable() {
    let transient_kinds = [
        std::io::ErrorKind::Interrupted,
        std::io::ErrorKind::WouldBlock,
        std::io::ErrorKind::TimedOut,
    ];
    for kind in transient_kinds {
        let err = ScraperError::Io(std::io::Error::new(kind, "transient io"));
        assert_eq!(
            err.classify(),
            ErrorClass::TransientRetriable,
            "io kind {kind:?} must classify TransientRetriable"
        );
    }
}

#[test]
fn classify_io_permanent_kind_is_permanent_fatal() {
    let permanent_kinds = [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::InvalidData,
    ];
    for kind in permanent_kinds {
        let err = ScraperError::Io(std::io::Error::new(kind, "permanent io"));
        assert_eq!(
            err.classify(),
            ErrorClass::PermanentFatal,
            "io kind {kind:?} must classify PermanentFatal"
        );
    }
}

#[test]
fn test_persistence_error_message() {
    let err = ScraperError::persistence("disco lleno");
    assert_eq!(err.to_string(), "Error de persistencia: disco lleno");
}

#[test]
fn test_ingestion_error_message() {
    let err = ScraperError::ingestion("pipeline abortó");
    assert_eq!(err.to_string(), "Error de ingestión: pipeline abortó");
}

// `rusqlite` is only linked under the `persistence` feature; this triangulation
// test must be gated to keep the default (core) build dependency-free.
#[cfg(all(feature = "persistence", not(miri)))]
#[test]
fn test_persistence_error_from_rusqlite() {
    // Triangulation: the Display-based helper must carry the real rusqlite
    // error text (proves it converts a genuine DB error, not a hardcoded value).
    let db = rusqlite::Connection::open_in_memory().expect("open in-memory sqlite");
    let rusqlite_err = db
        .prepare("SELECT * FROM tabla_inexistente")
        .expect_err("expected error for missing table");
    let scraper_err = ScraperError::persistence(&rusqlite_err);
    let msg = scraper_err.to_string();
    // Display contract: triangulation test — verify real rusqlite error text appears
    assert!(
        msg.contains("persistencia"),
        "missing Spanish prefix: {msg}"
    );
    assert!(
        msg.contains("no such table"),
        "missing rusqlite detail: {msg}"
    );
}

#[test]
fn test_semantic_error_chunk_too_large() {
    let err = SemanticError::ChunkTooLarge {
        chunk_id: "chunk-123".to_string(),
        chars: 600,
        max: 512,
    };
    assert!(
        matches!(
            err,
            SemanticError::ChunkTooLarge {
                chunk_id,
                chars: 600,
                max: 512,
            }
            if chunk_id == "chunk-123"
        ),
        "ChunkTooLarge fields must be preserved"
    );
}

#[test]
fn test_scraper_error_from_semantic() {
    let semantic_err = SemanticError::Tokenize("empty text".to_string());
    let scraper_err: ScraperError = semantic_err.into();
    assert!(
        matches!(
            scraper_err,
            ScraperError::Semantic(SemanticError::Tokenize(_))
        ),
        "SemanticError must convert to ScraperError::Semantic"
    );
}

/// #2338: the conversion fallback for `PartialRun` preserves the exact
/// rendered failure the same error produced as `Internal` before the
/// variant existed (programmatic consumers match the typed variant first).
#[test]
fn test_scraper_error_from_crawl_partial_run_preserves_message() {
    let err = CrawlError::PartialRun {
        message: "crawl stalled: test".to_string(),
        partial: crate::domain::CrawlResult::empty(),
    };
    let scraper_err: ScraperError = err.into();
    assert!(matches!(scraper_err, ScraperError::Internal(ref m) if m == "crawl stalled: test"));
}

/// The message must name what failed, not a runtime the crate no longer has.
#[test]
fn inference_message_does_not_name_onnx() {
    let msg = SemanticError::Inference("HTTP 503".to_string()).to_string();
    assert!(!msg.contains("ONNX"), "the message still names ONNX: {msg}");
    assert!(
        msg.contains("embeddings"),
        "the message must name the pipeline that failed: {msg}"
    );
}

#[test]
fn test_scraper_error_h2_config() {
    let err = ScraperError::H2Config("ALPN negotiation failed".to_string());
    assert!(
        matches!(err, ScraperError::H2Config(s) if s == "ALPN negotiation failed"),
        "H2Config variant with message must be preserved"
    );
}
