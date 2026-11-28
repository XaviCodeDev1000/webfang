//! OpenAI-compatible wreq adapter for `POST {base_url}/chat/completions` (#789).
//!
//! Wire shape: `response_format = {"type": "json_object"}` (unless the
//! provider sets `response_format_mode = "none"`, #3092), `temperature = 0.0`.
//! Error mapping reuses the existing [`ScraperError`] chain (zero new
//! variants): 429 → `Http{429}` (TransientBackoff), ≥500 → `Http{status}`
//! (TransientRetriable), transport → `Network`, malformed body / missing
//! choices → `Extraction`, `finish_reason == "length"` → `Validation`.

use crate::domain::credentials::ApiKey;
use crate::domain::llm_port::{ChatMessage, LlmPort, LlmRequest, LlmResponse};
use crate::domain::providers::{MaxTokensParameter, ResponseFormatMode};
use crate::error::{Result, ScraperError};
use crate::infrastructure::llm::http_client::build_guarded_http_client;

/// Maximum LLM completion body size (decompressed bytes). Completion payloads
/// are JSON well below this in practice; the cap bounds a broken or malicious
/// provider response. Closes audit finding F-R3-6 (AUDIT-02
/// rc3-closure-gate MATRIX.md): no fetch path may read an unbounded body.
const LLM_MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// Total attempts per completion: one initial send plus bounded retries.
const LLM_MAX_ATTEMPTS: u32 = 3;
/// Exponential backoff floor and ceiling, in milliseconds.
const LLM_BACKOFF_BASE_MS: u64 = 50;
const LLM_BACKOFF_MAX_MS: u64 = 1_000;
/// Ceiling for a server-supplied `Retry-After` (no indefinite wait).
const LLM_RETRY_AFTER_MAX_MS: u64 = 30_000;
/// Upper bound of the wall-clock jitter added to each backoff, in ms.
const LLM_JITTER_MAX_MS: u64 = 25;
use serde::Deserialize;
use serde_json::json;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
use url::Url;

/// Wire request for OpenAI-compatible `POST /chat/completions`.
///
/// `extra_body` is flattened at the body root and WebFang's fields are
/// declared last, so a vendor key can never replace `model`, `messages` or
/// the mapped completion parameters (#3074).
#[derive(serde::Serialize)]
struct CompletionRequest<'a> {
    #[serde(flatten)]
    extra_body: &'a crate::domain::providers::ExtraBody,
    model: &'a str,
    messages: &'a [ChatMessage],
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<serde_json::Value>,
    temperature: f32,
    /// Exactly one budget field is `Some`: [`CompletionWireOptions`] picks
    /// which one, so the two keys can never travel together (#3092).
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_completion_tokens: Option<usize>,
}

/// The fields this client writes itself, unless the provider opts out (#3092).
///
/// Defaults reproduce the pre-#3092 body byte for byte: `response_format` is
/// sent as `{"type": "json_object"}` and the budget travels as `max_tokens`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompletionWireOptions {
    /// Wire-level `response_format` policy.
    pub response_format_mode: ResponseFormatMode,
    /// Which wire key carries the budget; the numeric value is never rewritten.
    pub max_tokens_parameter: MaxTokensParameter,
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

/// Injected retry schedule. Tests replace waiting and jitter with deterministic
/// no-ops so attempt counts are observable without real time or randomness.
struct RetrySchedule {
    max_attempts: u32,
    base_ms: u64,
    max_ms: u64,
    jitter_ms: fn(u32) -> u64,
    sleep: fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>>,
}

impl Default for RetrySchedule {
    fn default() -> Self {
        Self {
            max_attempts: LLM_MAX_ATTEMPTS,
            base_ms: LLM_BACKOFF_BASE_MS,
            max_ms: LLM_BACKOFF_MAX_MS,
            jitter_ms: wall_jitter,
            sleep: real_sleep,
        }
    }
}

/// Small wall-clock jitter keeps concurrent clients from retrying in lockstep.
fn wall_jitter(attempt: u32) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    (u64::from(nanos) + u64::from(attempt)) % LLM_JITTER_MAX_MS
}

fn real_sleep(delay: Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(tokio::time::sleep(delay))
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
    /// Vendor-specific fields flattened into every completion body (#3074).
    extra_body: crate::domain::providers::ExtraBody,
    wire: CompletionWireOptions,
    retry: RetrySchedule,
}

impl OpenAiLlmClient {
    /// Build an adapter for `POST {base_url}/chat/completions`.
    ///
    /// # Errors
    ///
    /// Returns [`ScraperError::Config`] if the wreq client cannot be built.
    pub fn new(base_url: Url, api_key: ApiKey) -> Result<Self> {
        // SSRF guard (#703) applied through the domain `SsrfGuard` port:
        // literal-IP redirect guard + connect-time validating resolver that
        // re-validates every DNS answer. Entry-level `ssrf_gate` stays as
        // fast-fail typed UX; this is defense in depth.
        let client = build_guarded_http_client(false, None)
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
        Self::with_http_and_extra_body(client, base_url, api_key, serde_json::Map::new())
    }

    /// Same adapter with vendor-specific request fields. The composition root
    /// passes the provider's declared `extra_body`; tests that do not care
    /// about it keep using [`Self::with_http`].
    #[must_use]
    pub fn with_http_and_extra_body(
        client: wreq::Client,
        base_url: Url,
        api_key: ApiKey,
        extra_body: crate::domain::providers::ExtraBody,
    ) -> Self {
        Self {
            client,
            base_url,
            api_key,
            extra_body,
            wire: CompletionWireOptions::default(),
            retry: RetrySchedule::default(),
        }
    }

    /// Select which fields the client writes itself (#3092).
    ///
    /// Defaults are today's payload; the caller passes the provider's declared
    /// policy so a config option is never dead configuration.
    #[must_use]
    pub fn with_wire_options(mut self, wire: CompletionWireOptions) -> Self {
        self.wire = wire;
        self
    }

    /// The wire policy in force, so the composition layer can be pinned in a
    /// test.
    #[must_use]
    pub fn wire_options(&self) -> CompletionWireOptions {
        self.wire
    }

    /// One attempt: classify the status before reading any body. Terminal
    /// 4xx and exhausted budgets report the last observed status verbatim.
    async fn post_completion(
        &self,
        attempt: u32,
        endpoint: &str,
        wire: &CompletionRequest<'_>,
    ) -> Result<AttemptOutcome> {
        let response = self
            .client
            .post(endpoint)
            .bearer_auth(self.api_key.expose_secret())
            .json(wire)
            .send()
            .await
            .map_err(ScraperError::from)?;

        let status = response.status().as_u16();
        if response.status().is_success() {
            return Ok(AttemptOutcome::Success(Box::new(response)));
        }
        if !retryable_status(status) || attempt >= self.retry.max_attempts {
            return Ok(AttemptOutcome::Terminal(status));
        }
        Ok(AttemptOutcome::Retry {
            status,
            retry_after: retry_after_secs(&response),
        })
    }
}

/// Outcome of one HTTP attempt. Only 429 and 5xx produce `Retry`; a terminal
/// 4xx stops immediately and keeps its observed status.
enum AttemptOutcome {
    Success(Box<wreq::Response>),
    Retry {
        status: u16,
        retry_after: Option<u64>,
    },
    Terminal(u16),
}

/// Delta-seconds form of `Retry-After` (RFC 9110 §10.2.3); other forms and
/// hostile values degrade to bounded exponential backoff.
fn retry_after_secs(response: &wreq::Response) -> Option<u64> {
    let raw = response.headers().get("retry-after")?.to_str().ok()?;
    let secs = raw.trim().parse::<u64>().ok()?;
    (secs > 0).then_some(secs)
}

fn retryable_status(status: u16) -> bool {
    status == 429 || (500..=600).contains(&status)
}

/// Exponential backoff `base * 2^(attempt-1)` capped at `max_ms`, raised to a
/// server-supplied `Retry-After` (up to its own ceiling) and jittered. A retry
/// never arrives before the server asked.
fn retry_delay_ms(
    attempt: u32,
    retry_after: Option<u64>,
    jitter_ms: u64,
    base_ms: u64,
    max_ms: u64,
) -> u64 {
    let exponent = attempt.saturating_sub(1).min(20);
    let exponential = base_ms
        .saturating_mul(2_u64.saturating_pow(exponent))
        .min(max_ms);
    let honored = retry_after.map_or(0, |secs| {
        secs.saturating_mul(1_000).min(LLM_RETRY_AFTER_MAX_MS)
    });
    exponential
        .max(honored)
        .saturating_add(jitter_ms)
        .min(LLM_RETRY_AFTER_MAX_MS)
}

/// Parse a successful response; every failure here is an honest typed error.
async fn parse_response(response: wreq::Response) -> Result<LlmResponse> {
    let body = crate::domain::body_cap::read_body_capped(response, LLM_MAX_BODY_BYTES)
        .await
        .map_err(|e| {
            ScraperError::Extraction(format!("error al leer la respuesta del proveedor LLM: {e}"))
        })?;
    let parsed: CompletionResponse = serde_json::from_str(&body).map_err(|e| {
        ScraperError::Extraction(format!("el proveedor LLM devolvió un cuerpo inválido: {e}"))
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

    // `content: null` (tool-calls, content_filter) es Extraction honesto —
    // nunca silencio, nunca éxito con contenido vacío.
    let content = choice.message.content.clone().ok_or_else(|| {
        ScraperError::Extraction(
            "el proveedor LLM devolvió content null (tool-calls o content_filter)".to_string(),
        )
    })?;

    Ok(LlmResponse {
        content,
        input_tokens,
        output_tokens,
    })
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

            let (max_tokens, max_completion_tokens) = match self.wire.max_tokens_parameter {
                MaxTokensParameter::MaxTokens => (Some(request.max_tokens), None),
                MaxTokensParameter::MaxCompletionTokens => (None, Some(request.max_tokens)),
            };
            let wire = CompletionRequest {
                extra_body: &self.extra_body,
                model: &request.model,
                messages: &request.messages,
                response_format: match self.wire.response_format_mode {
                    ResponseFormatMode::JsonObject => Some(json!({ "type": "json_object" })),
                    ResponseFormatMode::None => None,
                },
                temperature: 0.0,
                max_tokens,
                max_completion_tokens,
            };

            let mut attempt = 0_u32;
            loop {
                attempt += 1;
                match self.post_completion(attempt, &endpoint, &wire).await? {
                    AttemptOutcome::Success(response) => return parse_response(*response).await,
                    AttemptOutcome::Terminal(status) => {
                        return Err(ScraperError::Http {
                            status,
                            // The configured base URL may carry inline
                            // credentials; the error text is an output sink.
                            url: crate::domain::providers::RedactedUrl::from_url(&self.base_url)
                                .to_string(),
                        });
                    },
                    AttemptOutcome::Retry {
                        status,
                        retry_after,
                    } => {
                        let jitter_ms = (self.retry.jitter_ms)(attempt);
                        let delay_ms = retry_delay_ms(
                            attempt,
                            retry_after,
                            jitter_ms,
                            self.retry.base_ms,
                            self.retry.max_ms,
                        );
                        tracing::warn!(
                            attempt,
                            status,
                            delay_ms,
                            max_attempts = self.retry.max_attempts,
                            "retrying LLM completion after transient provider response"
                        );
                        (self.retry.sleep)(Duration::from_millis(delay_ms)).await;
                    },
                }
            }
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

#[cfg(test)]
#[path = "client_wire_tests.rs"]
mod wire_tests;
