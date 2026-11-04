use super::*;
use crate::error::ErrorClass;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const VALID_BODY: &str = r#"{"choices":[{"message":{"content":"{\"items\":[]}"}}],
    "usage":{"prompt_tokens":11,"completion_tokens":7}}"#;

fn test_request() -> LlmRequest {
    LlmRequest {
        messages: vec![ChatMessage {
            role: "user".into(),
            content: "extract".into(),
        }],
        model: "stub-model".into(),
        max_tokens: 64,
    }
}

fn client_for(server: &MockServer) -> OpenAiLlmClient {
    OpenAiLlmClient::new(
        Url::parse(&server.uri()).expect("mock uri parses"),
        ApiKey::new("sk-test"),
    )
    .expect("client builds")
}

async fn mount(server: &MockServer, status: u16, body: &str) {
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

/// SSRF choke-point wiring proof (#1060): the client `OpenAiLlmClient::new`
/// actually builds must carry the guard obtained from the `SsrfGuard` port.
/// `localhost` resolves to loopback through getaddrinfo with no network
/// dependency, so the connect attempt must be rejected by the validating
/// resolver. (The wiremock tests above pass because their base URL is an
/// IP *literal*, which wreq resolves without consulting a custom resolver.)
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn llm_client_enforces_ssrf_guard_from_the_port() {
    // Env hermeticity (#926): the escape hatch is captured at client-build
    // time, so clearing it must be serialized against siblings that set it.
    // `EnvGuard` holds the shared process-env lock and restores on drop.
    let _env = webfang_test_utils::EnvGuard::clean(&[
        crate::domain::ssrf_guard::DISABLE_VALIDATING_RESOLVER_ENV,
    ]);
    let client = OpenAiLlmClient::new(
        Url::parse("http://localhost:9/").expect("loopback url parses"),
        ApiKey::new("sk-test"),
    )
    .expect("client builds");

    let err = client
        .client
        .get("http://localhost:9/")
        .send()
        .await
        .expect_err("hostname resolving to loopback must fail at connect");
    assert!(
        format!("{err:?}").contains("ForbiddenResolutionError"),
        "failure must come from the SSRF resolver, not the network: {err:?}"
    );
}

#[tokio::test]
async fn valid_json_response_returns_completion() {
    let server = MockServer::start().await;
    mount(&server, 200, VALID_BODY).await;
    let result = client_for(&server)
        .send_completion(test_request())
        .await
        .expect("200 + valid JSON succeeds");
    assert_eq!(result.content, r#"{"items":[]}"#);
    assert_eq!(result.input_tokens, 11);
    assert_eq!(result.output_tokens, 7);
}

#[tokio::test]
async fn malformed_body_maps_to_extraction_error() {
    let server = MockServer::start().await;
    mount(&server, 200, "<html>not json</html>").await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("non-JSON body must fail");
    assert!(
        matches!(err, ScraperError::Extraction(_)),
        "malformed body must be Extraction, got: {err:?}"
    );
}

#[tokio::test]
async fn missing_choices_maps_to_extraction_error() {
    let server = MockServer::start().await;
    mount(
        &server,
        200,
        r#"{"choices":[],"usage":{"prompt_tokens":1,"completion_tokens":1}}"#,
    )
    .await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("empty choices must fail");
    assert!(
        matches!(err, ScraperError::Extraction(_)),
        "empty choices must be Extraction, got: {err:?}"
    );
}

#[tokio::test]
async fn rate_limited_maps_to_transient_backoff() {
    let server = MockServer::start().await;
    mount(&server, 429, "{}").await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("429 must fail");
    assert!(
        matches!(&err, ScraperError::Http { status: 429, .. }),
        "429 must be Http{{429}}, got: {err:?}"
    );
    assert_eq!(err.classify(), ErrorClass::TransientBackoff);
}

#[tokio::test]
async fn server_error_maps_to_transient_retriable() {
    let server = MockServer::start().await;
    mount(&server, 503, "service down").await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("503 must fail");
    assert!(
        matches!(&err, ScraperError::Http { status: 503, .. }),
        "503 must be Http{{503}}, got: {err:?}"
    );
    assert_eq!(err.classify(), ErrorClass::TransientRetriable);
}

#[tokio::test]
async fn length_finish_reason_maps_to_validation_error() {
    let server = MockServer::start().await;
    let body = r#"{"choices":[{"message":{"content":"{\"items\":[]}"},
        "finish_reason":"length"}],"usage":{"prompt_tokens":1,"completion_tokens":64}}"#;
    mount(&server, 200, body).await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("truncated output must fail");
    assert!(
        matches!(err, ScraperError::Validation(_)),
        "finish_reason=length must be Validation, got: {err:?}"
    );
}

/// Fixture de contrato: el wire shape exacto que sale por el cable.
///
/// Golden en `fixtures/llm/chat_request.golden.json` (patrón `fixtures/waf`
/// + `waf_fixtures_test.rs`): el archivo vive en el repo, el diff del PR lo
/// muestra aislado y el revisor decide. Regla de actualización — escrita
/// aquí para que sobreviva al archivo que la contiene: **actualizar el
/// golden requiere commit separado con la justificación en el mensaje**.
/// "Arreglo test roto" en un commit de dos líneas mezclado con otros
/// cambios invalida la protección — el golden pasa a molestar sin proteger.
/// Cualquier cambio de serialización — campo agregado, renombrado,
/// `temperature` distinta — rompe este test ANTES de romper compatibilidad
/// con OpenAI/Ollama/vLLM en producción.
#[tokio::test]
async fn wire_shape_matches_contract_golden() {
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, Respond};

    struct Capture(Arc<Mutex<Option<serde_json::Value>>>);
    impl Respond for Capture {
        fn respond(&self, request: &wiremock::Request) -> ResponseTemplate {
            *self.0.lock().unwrap_or_else(|p| p.into_inner()) =
                Some(serde_json::from_slice(&request.body).expect("body es JSON"));
            ResponseTemplate::new(200).set_body_string(VALID_BODY)
        }
    }

    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<serde_json::Value>>> = Arc::new(Mutex::new(None));
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(Capture(captured.clone()))
        .mount(&server)
        .await;

    client_for(&server)
        .send_completion(test_request())
        .await
        .expect("200 + valid JSON succeeds");

    let body = captured
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
        .expect("el mock debió capturar el body");
    let golden_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/llm/chat_request.golden.json");
    let golden: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&golden_path).expect("golden legible"))
            .expect("golden es JSON válido");
    assert_eq!(
        body, golden,
        "wire shape cambió: ver regla de actualización en el doc-comment (commit separado + justificación)"
    );
}

/// Carga un fixture de respuesta desde `fixtures/llm/`.
fn load_response_fixture(name: &str) -> String {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../fixtures/llm/{name}"));
    std::fs::read_to_string(&path).expect("fixture legible")
}

/// Fixture de contrato (lado respuesta, cuerpo completo): los cuerpos
/// reales traen campos extra (`id`, `created`, `model`,
/// `system_fingerprint`, ...). El parse debe ignorarlos — fija que nadie
/// ponga `deny_unknown_fields` ni haga el parse estricto, lo que rompería
/// contra cualquier provider real.
#[tokio::test]
async fn provider_extra_response_fields_are_ignored() {
    let server = MockServer::start().await;
    mount(
        &server,
        200,
        &load_response_fixture("chat_response_full.json"),
    )
    .await;
    let result = client_for(&server)
        .send_completion(test_request())
        .await
        .expect("campos extra del provider no deben romper el parse");
    assert_eq!(result.content, r#"{"items":[]}"#);
    assert_eq!((result.input_tokens, result.output_tokens), (11, 7));
}

/// Fixture de contrato (lado respuesta, cuerpo parcial): aggregators y
/// vendors reales devuelven `content: null` (tool-calls, `content_filter`)
/// y `usage: null` (sin cómputo de uso). `#[serde(default)]` cubre
/// "ausente", NO "presente pero null" — `content: String` fallaba el parse
/// aquí. La decisión fijada: `content` es `Option`, `None` es `Extraction`
/// honesto (nunca silencio, nunca éxito vacío), `usage: null` da `(0, 0)`.
#[tokio::test]
async fn provider_null_content_maps_to_extraction_error() {
    let server = MockServer::start().await;
    mount(
        &server,
        200,
        &load_response_fixture("chat_response_partial.json"),
    )
    .await;
    let err = client_for(&server)
        .send_completion(test_request())
        .await
        .expect_err("content null debe fallar, no parsear en silencio");
    assert!(
        matches!(err, ScraperError::Extraction(_)),
        "content null debe ser Extraction, got: {err:?}"
    );
}

/// `usage` ausente (no solo null) también da `(0, 0)`: cubre vendors que
/// omiten el campo en vez de mandarlo null.
#[tokio::test]
async fn provider_missing_usage_gives_zero_tokens() {
    let server = MockServer::start().await;
    mount(
        &server,
        200,
        r#"{"choices":[{"message":{"content":"{\"items\":[]}"},"finish_reason":"stop"}]}"#,
    )
    .await;
    let result = client_for(&server)
        .send_completion(test_request())
        .await
        .expect("usage ausente no debe romper el parse");
    assert_eq!((result.input_tokens, result.output_tokens), (0, 0));
}
