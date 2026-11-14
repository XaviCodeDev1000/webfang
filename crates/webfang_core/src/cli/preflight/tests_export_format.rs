use super::*;

// ========================================================================
// #796 — --export-format vector preflight gate (mirrors #703/#652, #761)
// ========================================================================

/// Build opts with `--export-format vector`.
fn export_format_vector_opts(ai: bool) -> CrawlOptions {
    let mut opts = CrawlOptions {
        ai,
        ..CrawlOptions::default()
    };
    opts.export.export_format = ExportFormat::Vector;
    opts
}

/// `--export-format vector` WITHOUT `--clean-ai`: data-format error
/// (exit 65) with the Spanish message — mirrors the output-vectors gate
/// (#796).
///
/// The build-capability half of this gate disappeared with the `ai`
/// feature (ADR-0004 paso 6): there is no configuration left that cannot
/// produce embeddings, so the ONE remaining question is whether the
/// operator asked for vectors without asking for `--clean-ai`.
#[test]
fn export_format_vector_without_clean_ai_errors() {
    let opts = export_format_vector_opts(false);
    let err = check_export_format_vector(&opts)
        .expect_err("the gate must reject --export-format vector without --clean-ai");
    match err {
        CliExit::DataFormatError(msg) => assert!(
            msg.contains("No hay vectores para exportar") && msg.contains("--export-format vector"),
            "data format error must carry the Spanish message, got: {msg}"
        ),
        other => panic!("expected DataFormatError, got: {other:?}"),
    }
}

/// `--export-format vector` WITH `--clean-ai`: the embeddings will exist,
/// so the gate passes.
#[test]
fn export_format_vector_with_clean_ai_ok() {
    let opts = export_format_vector_opts(true);
    assert!(check_export_format_vector(&opts).is_ok());
}

/// `--export-format jsonl` (the default): never gated.
#[test]
fn export_format_jsonl_not_gated() {
    let opts = CrawlOptions::default();
    assert_eq!(opts.export.export_format, ExportFormat::Jsonl);
    assert!(check_export_format_vector(&opts).is_ok());
}

/// `--export-format auto`: never gated — it is not an explicit vector
/// request.
#[test]
fn export_format_auto_not_gated() {
    let mut opts = CrawlOptions::default();
    opts.export.export_format = ExportFormat::Auto;
    assert!(check_export_format_vector(&opts).is_ok());
}
