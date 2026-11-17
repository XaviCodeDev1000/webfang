use super::tests_support::test_handler;
use super::*;
use crate::mcp_server::handlers::test_support::result_text;
use crate::mcp_server::path_gate::host_abs;

#[tokio::test]
async fn export_file_empty_content_is_error() {
    let (handler, _tmp) = test_handler().await;
    // `output_dir` must be a safe relative path (params validation, #512).
    let out_dir = "test-output/export-empty";
    let _ = std::fs::remove_dir_all(out_dir);
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: out_dir.to_string(),
            filename: "doc".to_string(),
            content_format: "jsonl".to_string(),
            content: "   ".to_string(),
        }))
        .await
        .expect("export_file returns Ok on empty content");
    let json = serde_json::to_value(&res).expect("serialize");
    assert_eq!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "empty content must map to isError:true, got: {json}"
    );
    let _ = std::fs::remove_dir_all(out_dir);
}

#[tokio::test]
async fn export_file_invalid_format_is_invalid_params() {
    let (handler, _tmp) = test_handler().await;
    // #756: relative output_dir so the new root-of-trust gate passes and
    // the test still exercises the format parse error, not the gate.
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: "test-output/export-bad-format".to_string(),
            filename: "doc".to_string(),
            content_format: "bogus".to_string(),
            content: "hello".to_string(),
        }))
        .await;
    assert!(res.is_err(), "invalid format must be a protocol error");
}

/// Issue #601 regression: a `filename` with `..` must be rejected at the
/// validation boundary (protocol error), never written outside
/// `output_dir`.
#[tokio::test]
async fn export_file_rejects_filename_traversal() {
    let (handler, tmp) = test_handler().await;
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: tmp.path().to_string_lossy().to_string(),
            filename: "../escape".to_string(),
            content_format: "jsonl".to_string(),
            content: "hello".to_string(),
        }))
        .await;
    assert!(
        res.is_err(),
        "filename traversal '../escape' must be a protocol error"
    );
    // The file must NOT leak into the parent (repo root / CWD).
    let escaped = std::env::current_dir().expect("cwd").join("escape.jsonl");
    assert!(
        !escaped.exists(),
        "filename traversal wrote outside output_dir: {escaped:?}"
    );
}

/// Issue #601 regression: a `filename` containing a subdirectory separator
/// must be rejected (no silent nested-dir creation).
#[tokio::test]
async fn export_file_rejects_filename_subdirectory() {
    let (handler, tmp) = test_handler().await;
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: tmp.path().to_string_lossy().to_string(),
            filename: "sub/out".to_string(),
            content_format: "jsonl".to_string(),
            content: "hello".to_string(),
        }))
        .await;
    assert!(res.is_err(), "filename 'sub/out' must be a protocol error");
}

#[tokio::test]
async fn export_file_rejects_unknown_format_with_clear_message() {
    let (handler, _tmp) = test_handler().await;
    // Bug #4 regression: format="md" (unsupported) must return
    // invalid_params with a message listing supported formats (issue #590).
    // #756: relative output_dir so the new root-of-trust gate passes and
    // the test still exercises the format rejection, not the gate.
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: "test-output/export-unknown-format".to_string(),
            filename: "doc".to_string(),
            content_format: "md".to_string(),
            content: "hello".to_string(),
        }))
        .await;
    assert!(
        res.is_err(),
        "unsupported format 'md' must be a protocol error"
    );
}

#[tokio::test]
async fn export_file_writes_jsonl() {
    let (handler, _tmp) = test_handler().await;
    let out_dir = "test-output/export-jsonl";
    let _ = std::fs::remove_dir_all(out_dir);
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: out_dir.to_string(),
            filename: "doc".to_string(),
            content_format: "jsonl".to_string(),
            content: "hello world".to_string(),
        }))
        .await
        .expect("export_file returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("Exportación completada"),
        "success must report completion: {text}"
    );
    assert!(
        Path::new(out_dir).join("doc.jsonl").exists(),
        "export file must be written"
    );
    let _ = std::fs::remove_dir_all(out_dir);
}

/// PI-6 (#1601): caller-supplied `content` must be neutralized through
/// the provenance path BEFORE it reaches disk, and the written artifact
/// must carry the `provenance: caller-supplied` header at the top of the
/// caller payload. Asserted against the PARSED record: JSON escaping
/// would otherwise hide control characters from a byte-level check.
#[tokio::test]
async fn export_file_neutralizes_content_and_prefixes_provenance_header() {
    let (handler, _tmp) = test_handler().await;
    // Relative out_dir: the #756 root-of-trust gate rejects absolute
    // paths without configured export roots.
    let out_dir = "test-output/export-pi6";
    let _ = std::fs::remove_dir_all(out_dir);
    let res = handler
        .export_file(Parameters(ExportFileParams {
            output_dir: out_dir.to_string(),
            filename: "doc".to_string(),
            content_format: "jsonl".to_string(),
            content: "hello \u{1b}[31mred\u{1b}[0m world\u{7f}\nline2".to_string(),
        }))
        .await
        .expect("export_file returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("Exportación completada"),
        "neutralized export must still succeed: {text}"
    );

    let written = std::fs::read_to_string(Path::new(out_dir).join("doc.jsonl"))
        .expect("jsonl must be written");
    let record: serde_json::Value =
        serde_json::from_str(&written).expect("jsonl must stay parseable");
    let content = record
        .get("content")
        .and_then(|c| c.as_str())
        .expect("record must carry the content field");

    // The provenance header opens the caller payload, on its own line.
    assert!(
        content.starts_with("provenance: caller-supplied\n"),
        "provenance header must open the caller payload: {content:?}"
    );
    // ANSI escapes and DEL are gone; the readable text survives intact.
    assert!(
        !content.contains('\u{1b}'),
        "ANSI escape leaked: {content:?}"
    );
    assert!(!content.contains('\u{7f}'), "DEL leaked: {content:?}");
    assert!(
        content.contains("hello red world\nline2"),
        "visible text must survive neutralization: {content:?}"
    );
    let _ = std::fs::remove_dir_all(out_dir);
}

/// #756: `export_file` writes caller content to any `output_dir`, so an
/// absolute `output_dir` with no export roots configured must be rejected
/// at the protocol level (fail-closed root-of-trust gate, #696).
#[tokio::test]
async fn export_file_rejects_absolute_output_dir_without_roots() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .export_file(Parameters(ExportFileParams {
            // `host_abs`: host-appropriate absolute spelling (#1608) — a POSIX
            // literal is a root-without-prefix form on Windows hosts.
            output_dir: host_abs("/tmp/webfang-mcp-exploit"),
            filename: "doc".to_string(),
            content_format: "jsonl".to_string(),
            content: "hello".to_string(),
        }))
        .await;
    let err = res.expect_err("absolute output_dir without roots must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("requires server-configured export roots"),
        "error must name the missing export roots, got: {msg}"
    );
}
