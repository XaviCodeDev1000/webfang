use super::persistence::resolve_export_dir;
use super::{resolve_persistence_root, run};
use crate::application::crawl_options::CrawlOptions;
use crate::cli::error::CliExit;
use tracing_subscriber::layer::SubscriberExt;

// ===== #762 — persistence root / export dir convergence =====

/// No vault: both roots default to `-o`.
#[test]
fn persistence_root_defaults_to_output_dir() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("/tmp/out");

    assert_eq!(
        resolve_persistence_root(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
    assert_eq!(
        resolve_export_dir(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
}

/// quick_save: Markdown+assets root is the vault, RAG export keeps `-o`.
#[test]
fn quick_save_roots_to_vault_keeps_export_in_output_dir() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
    opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
    opts.export.quick_save = true;

    assert_eq!(
        resolve_persistence_root(&opts),
        std::path::PathBuf::from("/tmp/vault")
    );
    assert_eq!(
        resolve_export_dir(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
}

/// Explicit --vault (no quick_save): both roots redirect to the vault (#762).
#[test]
fn explicit_vault_redirects_both_roots_to_vault() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
    opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
    opts.export.vault_is_explicit = true;

    assert_eq!(
        resolve_persistence_root(&opts),
        std::path::PathBuf::from("/tmp/vault")
    );
    assert_eq!(
        resolve_export_dir(&opts),
        std::path::PathBuf::from("/tmp/vault")
    );
}

/// Config-filled vault without explicit flag: NO redirect (#762 — only
/// the explicit CLI flag changes the output base).
#[test]
fn config_filled_vault_does_not_redirect() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
    opts.export.obsidian_vault = Some(std::path::PathBuf::from("/tmp/vault"));
    opts.export.vault_is_explicit = false;

    assert_eq!(
        resolve_persistence_root(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
    assert_eq!(
        resolve_export_dir(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
}

/// Explicit flag with a missing vault falls back to `-o` — never panics.
#[test]
fn explicit_vault_without_path_falls_back_to_output_dir() {
    let mut opts = CrawlOptions::default();
    opts.export.output_dir = std::path::PathBuf::from("/tmp/out");
    opts.export.obsidian_vault = None;
    opts.export.vault_is_explicit = true;

    assert_eq!(
        resolve_persistence_root(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
    assert_eq!(
        resolve_export_dir(&opts),
        std::path::PathBuf::from("/tmp/out")
    );
}

// ===== output_vectors without clean-ai flag (#652, #703) =====

#[tokio::test]
async fn run_returns_data_error_when_output_vectors_without_clean_ai() {
    let mut opts = CrawlOptions::default();
    opts.elastic.output_vectors = Some("vectors.jsonl".to_string());
    // opts.ai stays false → no semantic cleaning requested

    // The cfg-gated `ai_cleaner` / `adaptive_engine` parameters make the
    // arity depend on both features — dispatch the call exactly like
    // `build_and_run` does in webfang_cli/src/main.rs.
    #[cfg(feature = "adaptive-selectors")]
    let exit = run(
        opts,
        None,
        None,
        crate::application::container::VaultAiPorts::default(),
        None,
    )
    .await;
    #[cfg(not(feature = "adaptive-selectors"))]
    let exit = run(
        opts,
        None,
        crate::application::container::VaultAiPorts::default(),
        None,
    )
    .await;

    assert!(
        matches!(exit, CliExit::DataFormatError(_)),
        "expected CliExit::DataFormatError, got {exit:?}"
    );
}

// ===== #1610 span identity =====

/// #1610 (OBS-H1): the root `run` span must carry the run-root identity it
/// mints for itself, not only the URL it was handed.
///
/// `#[instrument]` sees arguments only, and the root is a local minted
/// AFTER the span exists, so it is declared `Empty` and recorded in place.
/// The probe is a dry run with an unknown TLS profile: the run mints its
/// identity and returns at the profile check, before any socket opens —
/// deterministic, offline, and through the real entry point.
#[test]
fn run_span_carries_the_run_root_identity() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("trace.jsonl");
    let layer = crate::infrastructure::observability::FileTraceLayer::new(path.clone())
        .expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");

    let mut opts = CrawlOptions::default();
    opts.export.dry_run = true;
    opts.network.h2_profile = "NotAProfile".to_owned();
    let expected_url = opts.url.to_string();

    tracing::dispatcher::with_default(&dispatch, || {
        runtime.block_on(async {
            let exit = run(
                opts,
                None,
                #[cfg(feature = "adaptive-selectors")]
                None,
                Default::default(),
                None,
            )
            .await;
            assert!(
                matches!(exit, CliExit::ConfigError(_)),
                "an unknown TLS profile must abort before any fetch"
            );
        });
    });
    drop(runtime);
    // Flush before reading: the layer buffers and only drains on drop.
    drop(dispatch);

    let records: Vec<serde_json::Value> = std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .collect();

    let closes: Vec<_> = records
        .iter()
        .filter(|r| r["record"] == "span_close" && r["span"] == "run")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one root run span must close");
    let span_fields = closes[0]["span_fields"].as_object().expect("span_fields");
    let correlation_id = span_fields
        .get("correlation_id")
        .and_then(serde_json::Value::as_str)
        .expect("the root span must carry its own correlation_id");
    let trace_id = span_fields
        .get("trace_id")
        .and_then(serde_json::Value::as_str)
        .expect("the root span must carry its own trace_id");
    assert_eq!(
        span_fields.get("url").and_then(serde_json::Value::as_str),
        Some(expected_url.as_str()),
        "the declaration-time URL must survive the late records"
    );

    // The historical `run identity` event is the contract other tooling
    // reads; it must now carry the SAME identity the span does, so a
    // consumer can join on either one.
    let identity_events: Vec<_> = records
        .iter()
        .filter(|r| r["message"] == "run identity")
        .collect();
    assert_eq!(identity_events.len(), 1, "the run identity event stays");
    let event_fields = identity_events[0]["fields"]
        .as_object()
        .expect("event fields");
    assert_eq!(
        event_fields
            .get("correlation_id")
            .and_then(serde_json::Value::as_str),
        Some(correlation_id),
        "span and event must announce the same root"
    );
    assert_eq!(
        event_fields
            .get("trace_id")
            .and_then(serde_json::Value::as_str),
        Some(trace_id)
    );
}
