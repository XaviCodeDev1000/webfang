use super::tests_support::{seed_one, test_handler, test_state};
use super::*;
use crate::mcp_server::handlers::test_support::result_text;
use crate::mcp_server::path_gate::host_abs;

/// #756: same root-of-trust gate for `export_jsonl`. The gate runs before
/// the session read, so the rejection holds even with an empty session.
#[tokio::test]
async fn export_jsonl_rejects_absolute_output_dir_without_roots() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .export_jsonl(Parameters(ExportJsonlParams {
            output_dir: Some(host_abs("/tmp/webfang-mcp-exploit")),
            filename: Some("out".to_string()),
        }))
        .await;
    let err = res.expect_err("absolute output_dir without roots must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("requires server-configured export roots"),
        "error must name the missing export roots, got: {msg}"
    );
}

/// #756: same root-of-trust gate for `export_vector`.
#[tokio::test]
async fn export_vector_rejects_absolute_output_dir_without_roots() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .export_vector(Parameters(ExportVectorParams {
            output_dir: Some(host_abs("/tmp/webfang-mcp-exploit")),
            filename: Some("vec".to_string()),
        }))
        .await;
    let err = res.expect_err("absolute output_dir without roots must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("requires server-configured export roots"),
        "error must name the missing export roots, got: {msg}"
    );
}

/// #756: an absolute `output_dir` under a configured export root is
/// accepted — the gate is fail-closed for paths OUTSIDE every root, not
/// a blanket absolute-path ban (issue #600 compatibility).
#[tokio::test]
async fn export_jsonl_accepts_absolute_output_dir_inside_root() {
    let (state, tmp) = test_state().await;
    let root = tmp.path().to_path_buf();
    let state = state.with_export_roots(vec![root.clone()]);
    let handler = McpHandler::new(state);
    seed_one(&handler);

    let res = handler
        .export_jsonl(Parameters(ExportJsonlParams {
            output_dir: Some(root.to_string_lossy().to_string()),
            filename: Some("allowed".to_string()),
        }))
        .await
        .expect("export_jsonl returns Ok for absolute output_dir inside root");
    let text = result_text(&res);
    assert!(
        text.contains("Exportación completada"),
        "seeded export under a configured root must report completion: {text}"
    );
    assert!(
        root.join("allowed.jsonl").exists(),
        "export must be written under the configured root"
    );
}

#[tokio::test]
async fn export_jsonl_empty_session_is_error() {
    let (handler, _tmp) = test_handler().await;
    let res = handler
        .export_jsonl(Parameters(ExportJsonlParams {
            output_dir: None,
            filename: None,
        }))
        .await
        .expect("export_jsonl returns Ok on an empty session");
    let json = serde_json::to_value(&res).expect("serialize");
    assert_eq!(
        json.get("isError").and_then(|v| v.as_bool()),
        Some(true),
        "empty session must map to isError:true, got: {json}"
    );
}

#[tokio::test]
async fn export_jsonl_seeded_writes_file() {
    let (handler, _tmp) = test_handler().await;
    seed_one(&handler);
    let out_dir = "test-output/export-jsonl-seeded";
    let _ = std::fs::remove_dir_all(out_dir);
    let res = handler
        .export_jsonl(Parameters(ExportJsonlParams {
            output_dir: Some(out_dir.to_string()),
            filename: Some("out".to_string()),
        }))
        .await
        .expect("export_jsonl returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("Exportación completada"),
        "seeded export must report completion: {text}"
    );
    assert!(
        Path::new(out_dir).join("out.jsonl").exists(),
        "jsonl export file must be written"
    );
    let _ = std::fs::remove_dir_all(out_dir);
}

#[tokio::test]
async fn export_vector_seeded_writes_json() {
    let (handler, _tmp) = test_handler().await;
    seed_one(&handler);
    let out_dir = "test-output/export-vector-seeded";
    let _ = std::fs::remove_dir_all(out_dir);
    let res = handler
        .export_vector(Parameters(ExportVectorParams {
            output_dir: Some(out_dir.to_string()),
            filename: Some("vec".to_string()),
        }))
        .await
        .expect("export_vector returns Ok");
    let text = result_text(&res);
    assert!(
        text.contains("Exportación completada"),
        "seeded vector export must report completion: {text}"
    );
    assert!(
        Path::new(out_dir).join("vec.json").exists(),
        "vector export file must be written"
    );
    let _ = std::fs::remove_dir_all(out_dir);
}
