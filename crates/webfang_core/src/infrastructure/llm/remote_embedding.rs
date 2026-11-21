//! OpenAI-compatible wreq adapter for `POST {base_url}/embeddings` (#1462).
//!
//! Implements [`EmbeddingPort`] for
//! [`ProviderKind::OpenAiCompatible`](crate::domain::providers::ProviderKind)
//! so vault search works with non-384d dims. Since ADR-0004 paso 6 it is the
//! only embedding adapter with a production producer: it is selected through
//! the embedding provider slot (`--embedding-provider` / default resolution).
//!
//! Wire contract (`fixtures/remote_embedding/`):
//! - request `{model, input}` to `{base_url}/embeddings`, con cabecera
//!   `Authorization: Bearer` **sólo si** la fuente configurada aporta
//!   credencial: `auth: {source: "none"}` envía la petición anónima, sin
//!   sintetizar ni una cabecera vacía;
//! - response vectors map 1:1 to inputs in order;
//! - tolerant parse (never `deny_unknown_fields`, `usage` is `Option` +
//!   `#[serde(default)]`); missing `data` / empty vectors are
//!   [`SemanticError::Inference`], never silence.
//!
//! Guard chain (fetch-chain stages 1→5 minus crawl pacing, #1462):
//! entry gate at construction (scheme + parameterized literal-IP check) →
//! per-request timeout/connect timeout → 10-hop redirect policy → SSRF at
//! socket dial (parameterized loopback permit) → retry classification
//! (timeout/429/5xx retry, 4xx terminal, last status on exhaustion) →
//! `read_body_capped` at 16 MiB.

use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

use tracing::Instrument;
use url::Url;

use crate::domain::credentials::ApiKey;
use crate::domain::embedding_port::EmbeddingPort;
use crate::domain::providers::ProviderConfig;
use crate::error::SemanticError;
use crate::infrastructure::llm::provider::ProviderInitError;
use gate::entry_gate;
use wire::{EmbeddingResponse, EmbeddingUsage};

/// A boxed future for dyn-compatible async port methods.
type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Maximum remote-embedding response body (decompressed bytes), mirroring
/// `LLM_MAX_BODY_BYTES` (client.rs): no fetch path may read an unbounded
/// body.
const EMBEDDING_MAX_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// Bounded attempts for one `embed_batch`: the initial try plus retries for
/// timeouts, 429s and 5xx. Terminal 4xx and builder errors never consume a
/// retry.
const EMBEDDING_MAX_ATTEMPTS: u32 = 4;

/// Exponential backoff base/cap for embedding retries (production defaults
/// shared with the hardened HTTP client).
const BACKOFF_BASE_MS: u64 = 1000;
const BACKOFF_MAX_MS: u64 = 10_000;

/// Ceiling for a server-requested `Retry-After` delay.
///
/// `Retry-After` is server-controlled input and **both** wire forms accept
/// unbounded magnitudes: `delay-seconds` is a `u64` (`Duration::from_secs`
/// never fails) and an HTTP-date can sit centuries out. Unclamped, one header
/// pins the adapter in `tokio::time::sleep` for `u64::MAX` seconds ≈ 584M
/// years, and the millisecond conversion then saturates instead of erroring —
/// a stall, not a crash, which is the harder failure to diagnose. Real
/// embedding endpoints ask for at most ~60s, so 60s honors every legitimate
/// instruction while bounding the total stall across
/// [`EMBEDDING_MAX_ATTEMPTS`] retries to ~4 minutes.
const RETRY_AFTER_MAX_MS: u64 = 60_000;

/// Accumulated embedding usage metrics for a single run.
///
/// Collected by [`RemoteEmbeddingAdapter::metrics`] from the atomic counters
/// updated on each successful `embed_batch` call.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmbeddingRunMetrics {
    /// Total prompt tokens across all embedding requests in this run.
    pub prompt_tokens: u64,
    /// Total tokens (prompt + completion) across all embedding requests in this run.
    pub total_tokens: u64,
    /// Number of embedding requests made in this run.
    pub request_count: u64,
}

/// Remote embedding adapter: `EmbeddingPort` over `POST {base_url}/embeddings`.
///
/// The HTTP client always carries the parameterized SSRF guard
/// (`secure_client_with_loopback` with this provider's `allow_loopback`);
///
/// `dim` is set once by the startup probe ([`probe_dim`](Self::probe_dim)):
/// pin-verified when the config declares `embedding_dim`, adopted with an
/// info log otherwise. Before the probe runs, [`embedding_dim`](Self::embedding_dim)
/// reports the configured pin (or `0` when unpinned) — the adapter must be
/// probed at startup before serving, exactly like the completion provider is
/// validated before the first extraction.
pub struct RemoteEmbeddingAdapter {
    http: wreq::Client,
    base_url: Url,
    model: String,
    /// La credencial resuelta, o [`None`] cuando el provider declara
    /// `auth: {source: "none"}` — es decir, un endpoint anónimo por diseño.
    ///
    /// `Option` y no una `ApiKey` vacía: la ausencia de credencial se
    /// expresa como **ausencia de cabecera**, nunca como `Bearer ` — que un
    /// servidor puede rechazar como key inválida en vez de entender como
    /// petición anónima (invariante 2 de [`crate::domain::auth_source`]).
    api_key: Option<ApiKey>,
    /// Etiqueta de la fuente declarada (`"none"`, `"env"`, `"keyring"`,
    /// `"encrypted_file"`), para que un 401 pueda decir en el log si se
    /// mandó credencial o no. Nunca deriva del secreto.
    auth_source: &'static str,
    /// ADR-0021 proxy-auth provenance for the 407 diagnostic; never a secret.
    proxy_auth_configured: bool,
    provider_id: String,
    pinned_dim: Option<usize>,
    dim: OnceLock<usize>,
    /// Acumuladores de usage por corrida (thread-safe).
    /// prompt_tokens + total_tokens + request_count se incrementan en cada
    /// llamada exitosa a post_embeddings. Se leen vía `metrics()` al final
    /// de la corrida para el ScrapeSummary.
    usage_prompt_tokens: std::sync::atomic::AtomicU64,
    usage_total_tokens: std::sync::atomic::AtomicU64,
    usage_request_count: std::sync::atomic::AtomicU64,
}

/// Render an error together with its `source` chain.
///
/// The SSRF resolver's diagnosis (including the Spanish message that names
/// the missing `allow_hosts` field) lives at the END of the transport error's
/// source chain, not in its top-level `Display`: measured, a refused dial
/// renders as `error sending request for uri (…) → client error (Connect) →
/// dns error → <our message>`. Without walking the chain the operator sees
/// "Connect" and cannot tell a policy decision from a dead network.
///
/// Bounded by the chain itself, so it cannot loop.
///
/// Public because it is also the contract the tests assert against: an E2E
/// that proved "no guard denial" over `Display` alone would be vacuous, since
/// the verdict never appears there. Sharing one implementation is what keeps
/// the test honest rather than merely non-duplicated.
#[must_use]
pub fn render_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    let mut rendered = err.to_string();
    let mut source = err.source();
    while let Some(inner) = source {
        rendered.push_str(": ");
        rendered.push_str(&inner.to_string());
        source = inner.source();
    }
    rendered
}

/// Pure wire parse: response body → one vector per input, in order.
///
/// Tolerant by contract (unknown fields ignored); honest on degenerate
/// bodies — invalid JSON, missing `data`, empty `data`, an empty vector, or
/// a vector count that does not match the input count are all
/// [`SemanticError::Inference`]. The count check keeps chunk↔vector
/// alignment from silently shifting when a vendor truncates.
/// Result of parsing an embedding response body.
#[derive(Debug)]
struct ParsedEmbeddingBody {
    vectors: Vec<Vec<f32>>,
    usage: Option<EmbeddingUsage>,
}

/// Pure wire parse: response body → vectors + optional usage, with 1:1 count check.
///
/// Tolerant by contract (unknown fields ignored); honest on degenerate
/// bodies — invalid JSON, missing `data`, empty `data`, an empty vector, or
/// a vector count that does not match the input count are all
/// [`SemanticError::Inference`]. The count check keeps chunk↔vector
/// alignment from silently shifting when a vendor truncates.
fn parse_embedding_body(
    body: &str,
    expected_inputs: usize,
) -> Result<ParsedEmbeddingBody, SemanticError> {
    let parsed: EmbeddingResponse = serde_json::from_str(body).map_err(|e| {
        SemanticError::Inference(format!(
            "el endpoint de embeddings devolvió un cuerpo inválido: {e}"
        ))
    })?;
    if parsed.data.is_empty() {
        return Err(SemanticError::Inference(
            "el endpoint de embeddings devolvió `data` vacío".to_string(),
        ));
    }
    if parsed.data.len() != expected_inputs {
        return Err(SemanticError::Inference(format!(
            "el endpoint de embeddings devolvió {} vectores para {expected_inputs} textos",
            parsed.data.len()
        )));
    }
    let mut vecs = Vec::with_capacity(parsed.data.len());
    for datum in &parsed.data {
        if datum.embedding.is_empty() {
            return Err(SemanticError::Inference(
                "el endpoint de embeddings devolvió un vector vacío".to_string(),
            ));
        }
        vecs.push(datum.embedding.clone());
    }
    Ok(ParsedEmbeddingBody {
        vectors: vecs,
        usage: parsed.usage,
    })
}

impl RemoteEmbeddingAdapter {
    /// Variant with an already-built HTTP client (shared process client or
    /// wiremock in tests).
    ///
    /// # SSRF contract
    ///
    /// The caller owns the injected client's protection: production passes
    /// the client from [`new`](Self::new) (parameterized guard applied);
    /// tests inject a plain client against loopback wiremock.
    ///
    /// # Errors
    ///
    /// [`ProviderInitError::InvalidBaseUrl`] / `AuthFailed` — the entry gate
    /// and the credential resolution happen here, NOT on the first call.
    pub fn with_http(
        config: ProviderConfig,
        http: wreq::Client,
    ) -> Result<Self, ProviderInitError> {
        Self::with_http_and_proxy_auth(config, http, false)
    }

    /// Provider identifier (startup diagnostics and error context).
    #[must_use]
    pub fn id(&self) -> &str {
        &self.provider_id
    }

    /// Base endpoint of the provider (`POST {base_url}/embeddings`).
    ///
    /// Read-only: exposes the configured endpoint for audit logging (e.g. the
    /// Tier 2 consent hash in the CLI) without leaking the credential.
    #[must_use]
    pub fn base_url(&self) -> &Url {
        &self.base_url
    }

    /// Startup probe: pin-or-adopt the served dimension.
    ///
    /// POSTs `["probe"]` and reads the actual dim off the first vector: a
    /// declared `embedding_dim` that disagrees fails closed (mixed dims
    /// silently break cosine ranking — no auto-migration); unpinned adopts
    /// with an info log and serves it from here on.
    ///
    /// Call once at startup before serving; the binary maps failures to
    /// exit-78 `ConfigError`.
    ///
    /// # Errors
    ///
    /// [`SemanticError::Inference`] when the probe request fails, the body
    /// is degenerate, or a declared pin disagrees with the served dim.
    pub async fn probe_dim(&self) -> Result<usize, SemanticError> {
        let probe = vec!["probe".to_string()];
        let actual = self
            .post_embeddings(&probe)
            .await?
            .into_iter()
            .next()
            .map(|vec| vec.len())
            .unwrap_or(0);
        if let Some(pinned) = self.pinned_dim {
            if actual != pinned {
                return Err(SemanticError::Inference(format!(
                    "dimensión declarada {pinned} != remota {actual} para provider '{}'",
                    self.provider_id
                )));
            }
        } else {
            tracing::info!(
                provider_id = %self.provider_id,
                dim = actual,
                "remote embedding dim adopted from startup probe"
            );
        }
        let _ = self.dim.set(actual);
        Ok(actual)
    }

    /// The dimension this adapter serves: probed once available, else the
    /// configured pin, else `0` (cold adapter — probe at startup before
    /// serving).
    #[must_use]
    pub fn embedding_dim(&self) -> usize {
        self.dim.get().copied().or(self.pinned_dim).unwrap_or(0)
    }
}

impl EmbeddingPort for RemoteEmbeddingAdapter {
    fn embed<'a>(&'a self, text: &'a str) -> BoxFuture<'a, Result<Vec<f32>, SemanticError>> {
        let request_id = uuid::Uuid::new_v4();
        let span = tracing::debug_span!(
            "embed",
            provider_id = %self.provider_id,
            model = %self.model,
            dim = self.embedding_dim(),
            request_id = %request_id
        );
        Box::pin(
            async move {
                let batch = self
                    .embed_batch_impl(std::slice::from_ref(&text.to_owned()), request_id)
                    .await?;
                batch.into_iter().next().ok_or_else(|| {
                    SemanticError::Inference(
                        "el endpoint de embeddings devolvió `data` vacío".to_string(),
                    )
                })
            }
            .instrument(span),
        )
    }

    fn embed_batch<'a>(
        &'a self,
        texts: &'a [String],
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>, SemanticError>> {
        let request_id = uuid::Uuid::new_v4();
        Box::pin(self.embed_batch_impl(texts, request_id))
    }

    fn embedding_dim(&self) -> usize {
        self.embedding_dim()
    }

    /// The provider-declared model name — exactly what every embeddings
    /// request carries, so it is the identity a consumer can re-derive.
    fn model_id(&self) -> &str {
        &self.model
    }

    /// Returns the concrete adapter for downcasting.
    ///
    /// Required, not optional: the trait default **panics** and the CLI reaches
    /// these counters through a vtable dispatch (`Arc<dyn EmbeddingPort>::as_any`).
    /// A private inherent method of the same name never enters the vtable: the
    /// call lands on the panicking default and aborts the run.
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Private implementation with an explicit per-request id for span propagation.
/// Not part of the trait — called directly on the concrete type.
///
/// The id is published as `request_id`, never as `correlation_id` (#2672).
impl RemoteEmbeddingAdapter {
    async fn embed_batch_impl(
        &self,
        texts: &[String],
        request_id: uuid::Uuid,
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let span = tracing::debug_span!(
            "embed_batch",
            provider_id = %self.provider_id,
            model = %self.model,
            count = texts.len(),
            dim = self.embedding_dim(),
            request_id = %request_id
        );
        async move {
            if texts.is_empty() {
                return Ok(Vec::new());
            }
            self.post_embeddings(texts).await
        }
        .instrument(span)
        .await
    }

    /// Returns the accumulated embedding usage metrics for this run.
    ///
    /// Includes prompt_tokens, total_tokens, and request_count since the
    /// adapter was created (or since startup). Thread-safe via AtomicU64.
    #[must_use]
    pub fn metrics(&self) -> EmbeddingRunMetrics {
        EmbeddingRunMetrics {
            prompt_tokens: self
                .usage_prompt_tokens
                .load(std::sync::atomic::Ordering::Relaxed),
            total_tokens: self
                .usage_total_tokens
                .load(std::sync::atomic::Ordering::Relaxed),
            request_count: self
                .usage_request_count
                .load(std::sync::atomic::Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod fixtures;
mod gate;
mod http;
mod proxy_auth;
mod proxy_wiring;
mod retry;
mod wire;

#[cfg(test)]
mod adapter_tests;
#[cfg(test)]
mod e2e_tests;

#[cfg(test)]
mod tests {
    use super::fixtures::adapter_for;
    use super::*;

    // --- Trait contract through the vtable ---

    #[tokio::test]
    async fn as_any_dispatches_through_the_vtable() {
        // Regression pin: `EmbeddingPort::as_any` panics by default, so an
        // impl carrying only a private *inherent* method of the same name
        // still resolves to that panicking default when called through
        // `Arc<dyn EmbeddingPort>`. That shape aborted the CLI's embedding
        // summary after a Tier 2 run had already emitted its output.
        let server = wiremock::MockServer::start().await;
        let adapter = adapter_for(&server, |_| {});

        // Erase the concrete type exactly like the caller does, so the
        // dispatch below must reach the trait impl and not an inherent one.
        let erased: std::sync::Arc<dyn EmbeddingPort> = std::sync::Arc::new(adapter);

        let adapter = erased
            .as_any()
            .downcast_ref::<RemoteEmbeddingAdapter>()
            .expect("as_any must dispatch to the trait impl, not the panicking default");
        // Reaching `metrics()` is the reason this downcast exists at all.
        let metrics = adapter.metrics();
        assert_eq!(
            metrics.request_count, 0,
            "a freshly built adapter has issued no requests"
        );
    }
}
