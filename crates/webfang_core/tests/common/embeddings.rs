//! A fake REMOTE embeddings provider for the CLI's `--clean-ai` path.
//!
//! ADR-0004 removed the local ONNX stack, so `--clean-ai` now REQUIRES a
//! configured REMOTE embedding provider (preflight #796) and `--export-format
//! vector` requires `--clean-ai`. A test that exercises the vector export
//! therefore has to PROVIDE one — the two tests that used to be `#[ignore]`d
//! for "requires cached ONNX model" / "requires a configured remote embedding
//! provider" were waiting for exactly this seam (#2666).
//!
//! Hermetic by construction: the endpoint is the test's own wiremock server,
//! the credential is anonymous (`auth: { source = "none" }`) and nothing
//! leaves the machine. The provider config is written to a temp file and
//! handed to the binary through `WEBFANG_CONFIG` (the CLI refuses a relative
//! path).

use std::path::PathBuf;

use wiremock::matchers::{method, path};
use wiremock::{Mock, Request, Respond, ResponseTemplate};

/// The provider id the CLI is pointed at with `--embedding-provider`.
pub const PROVIDER_ID: &str = "test-embeddings";

/// The model the fake endpoint serves.
const MODEL: &str = "test-embedding-model";

/// The dimension every fake vector carries. The adapter PROBES the served
/// dimension at startup, so the probe and the real call must agree.
const DIM: usize = 8;

/// Mount the OpenAI-compatible embeddings endpoint on `server`.
///
/// The body carries ONE VECTOR PER INPUT of the request: the adapter rejects a
/// body whose vector count does not match the text count, so a static body
/// would break as soon as a page chunks into more than one text.
pub async fn mount_embeddings(server: &wiremock::MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(EmbeddingResponder)
        .mount(server)
        .await;
}

/// Write the provider config the CLI reads and return its ABSOLUTE path.
///
/// The provider points at `server`'s own authority with `allow_loopback`: the
/// endpoint is `127.0.0.1` (wiremock binds an ephemeral port), and the
/// loopback opt-in is a config-file-only knob by design (#1462) — there is
/// deliberately no CLI flag for it.
pub fn provider_config(server: &wiremock::MockServer) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "webfang-embeddings-config-{}-{n}.toml",
        std::process::id()
    ));
    let contents = format!(
        "[[providers]]\n\
         id = \"{PROVIDER_ID}\"\n\
         display_name = \"Embeddings de prueba\"\n\
         kind = \"open_ai_compatible\"\n\
         base_url = \"{}/v1\"\n\
         auth = {{ source = \"none\" }}\n\
         capabilities = [\"embedding\"]\n\
         model = \"{MODEL}\"\n\
         allow_loopback = true\n",
        server.uri()
    );
    std::fs::write(&path, contents).expect("write the embeddings provider config");
    path
}

/// Answers the embeddings request with one vector per input text.
struct EmbeddingResponder;

impl Respond for EmbeddingResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let inputs = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|body| {
                body["input"]
                    .as_array()
                    .map(|items| items.len())
                    .or_else(|| body["input"].as_str().map(|_| 1))
            })
            .unwrap_or(1);
        let data: Vec<serde_json::Value> = (0..inputs)
            .map(|index| {
                serde_json::json!({
                    "object": "embedding",
                    "index": index,
                    "embedding": vec![0.1_f32; DIM],
                })
            })
            .collect();
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": data,
            "model": MODEL,
            "usage": { "prompt_tokens": 1, "total_tokens": 1 },
        }))
    }
}
