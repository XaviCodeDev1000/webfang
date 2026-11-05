//! OpenAI-compatible wreq adapter for `POST {base_url}/chat/completions` (#789).
//!
//! Wire shape: `response_format = {"type": "json_object"}`, `temperature = 0.0`.
//! Error mapping reuses the existing [`ScraperError`] chain (zero new
//! variants): 429 → `Http{429}` (TransientBackoff), ≥500 → `Http{status}`
//! (TransientRetriable), transport → `Network`, malformed body / missing
//! choices → `Extraction`, `finish_reason == "length"` → `Validation`.

use crate::domain::credentials::ApiKey;
use crate::domain::llm_port::{ChatMessage, LlmPort, LlmRequest, LlmResponse};
use crate::error::{Result, ScraperError};

/// Maximum LLM completion body size (decompressed bytes). Completion payloads
/// are JSON well below this in practice; the cap bounds a broken or malicious
/// provider response. Closes audit finding F-R3-6 (AUDIT-02
/// rc3-closure-gate MATRIX.md): no fetch path may read an unbounded body.
const LLM_MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;

/// Wire request for OpenAI-compatible `POST /chat/completions`.
#[derive(serde::Serialize)]
struct CompletionRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    response_format: serde_json::Value,
    temperature: f32,
    max_tokens: usize,
}

/// Wire response: only the fields this client consumes.
///
/// Tolerancia deliberada (fixtures de contrato en `fixtures/llm/`):
/// - `content` es `Option`: tool-calls y `content_filter` devuelven
///   `content: null` presente — `String` fallaría el parse donde `Option`
///   lo tolera. Un `None` aquí es `Extraction` honesto en `send_completion`,
///   nunca silencio.
/// - `usage` ya es `Option` + `#[serde(default)]`: vendors que no computan
///   uso devuelven `usage: null` o lo omiten — ambos dan `(0, 0)`.
/// - Sin `deny_unknown_fields` jamás: los cuerpos reales traen `id`,
///   `created`, `system_fingerprint`, `logprobs`, `total_tokens`, ...
#[derive(Deserialize)]
struct CompletionResponse {
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// OpenAI-compatible chat/completions adapter (covers OpenAI / Ollama / vLLM).
///
/// The client carries no SSRF logic of its own: `new` applies the
/// domain-owned [`crate::domain::ssrf_guard::SsrfGuard`] port (#703), while
/// the application-layer
/// [`crate::application::llm_extraction::ssrf_gate`] validates the base URL
/// before any request is sent. Unit tests call the client directly against
/// wiremock (loopback) by design.
pub struct OpenAiLlmClient {
    client: wreq::Client,
    base_url: Url,
    api_key: ApiKey,
}

impl OpenAiLlmClient {
    /// Build an adapter for `POST {base_url}/chat/completions`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] if the wreq client cannot be built.
    pub fn new(base_url: Url, api_key: ApiKey) -> Result<Self> {
        let builder = wreq::Client::builder()
            .emulation(wreq_util::Profile::Chrome145)
            .timeout(Duration::from_secs(60))
            .connect_timeout(Duration::from_secs(10));
        // SSRF guard (#703) applied through the domain `SsrfGuard` port:
        // literal-IP redirect guard + connect-time validating resolver that
        // re-validates every DNS answer. Entry-level `ssrf_gate` stays as
        // fast-fail typed UX; this is defense in depth.
        let client = crate::domain::ssrf_guard::ssrf_guard()
            .secure_client(builder)
            .build()
            .map_err(|e| ScraperError::Config(format!("no se pudo crear el cliente LLM: {e}")))?;
        Ok(Self::with_http(client, base_url, api_key))
    }

    /// Variante con un cliente HTTP ya construido (cliente compartido del
    /// proceso o wiremock en tests).
    ///
    /// # Contrato SSRF
    ///
    /// El caller es responsable de que el cliente inyectado esté protegido:
    /// `build_default_http_client` (provider) aplica el `SsrfGuard`; los tests
    /// que apuntan a wiremock loopback usan los hatches de `webfang_test_utils`.
    #[must_use]
    pub fn with_http(client: wreq::Client, base_url: Url, api_key: ApiKey) -> Self {
        Self {
            client,
            base_url,
            api_key,
        }
    }
}

impl LlmPort for OpenAiLlmClient {
    fn send_completion<'a>(
        &'a self,
        request: LlmRequest,
    ) -> Pin<Box<dyn Future<Output = Result<LlmResponse>> + Send + 'a>> {
        Box::pin(async move {
            let endpoint = format!(
                "{}/chat/completions",
                self.base_url.as_str().trim_end_matches('/')
            );

            let wire = CompletionRequest {
                model: &request.model,
                messages: &request.messages,
                response_format: json!({ "type": "json_object" }),
                temperature: 0.0,
                max_tokens: request.max_tokens,
            };

            let response = self
                .client
                .post(endpoint)
                .bearer_auth(self.api_key.expose_secret())
                .json(&wire)
                .send()
                .await
                .map_err(ScraperError::from)?;

            let status = response.status().as_u16();
            if !response.status().is_success() {
                return Err(ScraperError::Http {
                    status,
                    url: self.base_url.to_string(),
                });
            }

            let body = crate::domain::body_cap::read_body_capped(response, LLM_MAX_BODY_BYTES)
                .await
                .map_err(|e| {
                    ScraperError::Extraction(format!(
                        "error al leer la respuesta del proveedor LLM: {e}"
                    ))
                })?;
            let parsed: CompletionResponse = serde_json::from_str(&body).map_err(|e| {
                ScraperError::Extraction(format!(
                    "el proveedor LLM devolvió un cuerpo inválido: {e}"
                ))
            })?;

            let choice = parsed.choices.first().ok_or_else(|| {
                ScraperError::Extraction("el proveedor LLM no devolvió choices".to_string())
            })?;

            if choice.finish_reason.as_deref() == Some("length") {
                return Err(ScraperError::Validation(
                    "la salida del LLM fue truncada por el límite de tokens (finish_reason=length)"
                        .to_string(),
                ));
            }

            let (input_tokens, output_tokens) = parsed
                .usage
                .map(|u| (u.prompt_tokens, u.completion_tokens))
                .unwrap_or((0, 0));

            // `content: null` (tool-calls, content_filter) es Extraction
            // honesto — nunca silencio, nunca éxito con contenido vacío.
            let content = choice.message.content.clone().ok_or_else(|| {
                ScraperError::Extraction(
                    "el proveedor LLM devolvió content null (tool-calls o content_filter)"
                        .to_string(),
                )
            })?;

            Ok(LlmResponse {
                content,
                input_tokens,
                output_tokens,
            })
        })
    }
}

// Test-module wiring for a FILE-module parent: `#[path]` points at the
// sibling file explicitly while preserving the `client::tests` module path.
// Directory-module parents (e.g. `scraper/mod.rs`) declare their test
// siblings as plain `mod` items instead; both forms are valid, and the
// difference is the parent's shape (issue #2614).
#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
