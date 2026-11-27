//! `OpenAiCompatibleProvider` — composition of `ProviderConfig` + shared HTTP
//! client + resolved credential (doc `ai-providers-design.md`).
//!
//! Contract fixed in review:
//! - `AuthSource::resolve()` resolves ONCE in the constructor: it fails at
//!   startup, not on the first call.
//! - `AuthError` converts into [`ProviderInitError`] with `provider_id` in the
//!   message — it is never propagated raw up to the Container.
//! - The HTTP client is shared process-wide via
//!   [`build_default_http_client`]; the provider accepts one with `with_http`.
//! - `AuthSource::None` resolves to `Ok(None)`: this is **not** a credential
//!   failure. See [`ProviderInitError::AnonymousCompletionUnsupported`] for why
//!   the completion path does not support it yet.

use crate::domain::auth_source::AuthError;
use crate::domain::llm_port::LlmPort;
use crate::domain::proxy_config::ProxyRegistry;
use crate::infrastructure::llm::client::OpenAiLlmClient;
use crate::infrastructure::llm::http_client::{build_guarded_http_client, build_llm_client};

/// Declarative provider configuration: `crate::domain::providers::ProviderConfig`
/// (with `kind`, `capabilities`, `model`, `embedding_dim`). Re-exported here as
/// `ProviderConfig` so existing import paths keep working.
pub use crate::domain::providers::ProviderConfig;

/// Provider initialization error.
///
/// Converts `AuthError` while preserving context (`provider_id`) so the
/// Container and reports do not lose the origin. Messages are user-facing
/// (in English, per repo convention).
#[derive(Debug, thiserror::Error)]
pub enum ProviderInitError {
    /// The credential could not be resolved while building the provider (a
    /// startup failure, not a first-call failure).
    #[error("could not resolve credential for provider '{provider_id}': {source}")]
    AuthFailed {
        /// Identifier of the provider that failed (Container context).
        provider_id: String,
        /// Original resolution error (source preserved).
        #[source]
        source: AuthError,
    },
    /// Invalid base URL. Reserved for configuration loading (parsing the URL
    /// from a string in the wizard/container); the constructor taking an
    /// already-validated `Url` never produces it.
    #[error("invalid base URL for provider '{provider_id}': {source}")]
    InvalidBaseUrl {
        /// Identifier of the provider whose URL failed.
        provider_id: String,
        /// Original parse/construction error.
        #[source]
        source: crate::error::ScraperError,
    },
    /// The shared HTTP client constructor failed (network/TLS config).
    #[error("could not build the shared HTTP client: {0}")]
    HttpClient(String),
    /// The provider declares `auth: {source: none}` on the chat/completions
    /// path, which still requires a credential.
    ///
    /// **Transient variant** (B1): the anonymous endpoint is already supported
    /// for remote embeddings (`RemoteEmbeddingAdapter`), but the completion
    /// client (`OpenAiLlmClient`) stores an `ApiKey` and calls `bearer_auth`
    /// unconditionally, so it cannot represent "no credential". That file
    /// (`infrastructure/llm/client.rs`) is outside B1's surfaces and is
    /// requested as a separate patch.
    ///
    /// Deliberately **NOT** [`AuthFailed`](Self::AuthFailed): `None` is not a
    /// credential failure, and mapping it there would tell the user to rotate a
    /// key they never declared.
    #[error("provider '{provider_id}': anonymous endpoint (`auth: none`) is not yet supported by the chat/completions path")]
    AnonymousCompletionUnsupported {
        /// Identifier of the provider configured as anonymous.
        provider_id: String,
    },
}

/// Configuration re-exported above: `crate::domain::providers::ProviderConfig`.
/// Build the process-wide shared HTTP client for providers.
///
/// A single `wreq::Client` (Chrome145, timeouts, SSRF guard) shared by every
/// provider: connection pooling and one TLS-fingerprint surface. It returns the
/// bare client — each provider composes it with its own base_url/api_key.
///
/// # Errors
///
/// [`ProviderInitError::HttpClient`] if the builder fails.
pub fn build_default_http_client() -> Result<wreq::Client, ProviderInitError> {
    build_guarded_http_client(false, None).map_err(|e| ProviderInitError::HttpClient(e.to_string()))
}

/// OpenAI-compatible provider, ready to be injected into the Container as
/// `Arc<dyn LlmPort>`.
///
/// Deliberately no `Debug`: it holds the resolved credential (via the client).
/// Diagnostics use `config()` + `id()`, never the secret.
///
/// `None` policy (Container contract): the Container is permissive — the
/// absence of a provider does NOT fail construction. But permissive does not
/// mean optional: every service binary starting with an LLM feature enabled
/// MUST validate `llm_port().is_some()` at startup and exit with a clear error
/// when it is missing. If no binary validates, the failure arrives late on the
/// first call (`ScraperError::Config` in `extract`) — a daemon that says OK at
/// boot and fails when a user or cron invokes the feature, with retries
/// amplifying before anyone notices.
/// The Container never invents a provider nor degrades silently:
/// `llm_port()` stays `None` until the binary layer injects one.
pub struct OpenAiCompatibleProvider {
    client: OpenAiLlmClient,
    config: ProviderConfig,
    /// ADR-0021: whether the proxy hop had a configured credential. Used only
    /// by the 407 diagnostic; the secret itself is never retained here.
    proxy_auth_configured: bool,
}

impl OpenAiCompatibleProvider {
    /// Builds the provider, resolving the credential ONCE (at startup).
    ///
    /// Uses the process-wide shared HTTP client
    /// ([`build_default_http_client`]). To reuse an existing client
    /// (tests, wiremock), use [`with_http`](Self::with_http).
    ///
    /// # Errors
    ///
    /// [`ProviderInitError`] if the credential does not resolve, the URL is
    /// invalid, or the shared client cannot be built.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
        Self::new_with_registry(config, &ProxyRegistry::default())
    }

    /// ADR-0021 variant: builds the client from the provider's optional proxy
    /// declaration resolved against `registry`. A provider that declares a
    /// proxy but is built without a catalog fails loud (unknown reference),
    /// never silently falling back to direct egress.
    ///
    /// # Errors
    ///
    /// [`ProviderInitError`] for a rejected proxy, an unresolved credential,
    /// or a builder failure — and every error [`Self::with_http`] reports.
    pub fn new_with_registry(
        config: ProviderConfig,
        registry: &ProxyRegistry,
    ) -> Result<Self, ProviderInitError> {
        let bundle = build_llm_client(&config, registry)
            .map_err(|e| ProviderInitError::HttpClient(e.to_string()))?;
        Self::with_http_and_proxy_auth(config, bundle.client, bundle.proxy_auth_configured)
    }

    /// Variant with an injected HTTP client (composition root/tests).
    ///
    /// # Errors
    ///
    /// [`ProviderInitError::AuthFailed`] / `InvalidBaseUrl` — the credential
    /// is resolved here, NOT on the first call.
    /// [`ProviderInitError::AnonymousCompletionUnsupported`] if the provider
    /// declares `auth: none` (see the variant: the completion client still
    /// requires a credential).
    pub fn with_http(
        config: ProviderConfig,
        http: wreq::Client,
    ) -> Result<Self, ProviderInitError> {
        Self::with_http_and_proxy_auth(config, http, false)
    }

    /// ADR-0021: records proxy-auth provenance for the 407 diagnostic.
    ///
    /// # Errors
    ///
    /// Same as [`Self::with_http`].
    pub fn with_http_and_proxy_auth(
        config: ProviderConfig,
        http: wreq::Client,
        proxy_auth_configured: bool,
    ) -> Result<Self, ProviderInitError> {
        let secret = config
            .auth
            .resolve()
            .map_err(|source| ProviderInitError::AuthFailed {
                provider_id: config.id.clone(),
                source,
            })?;
        // `OpenAiLlmClient` stores an `ApiKey` and signs every request
        // (`bearer_auth` unconditionally in `client.rs`). Without the change
        // --`api_key: Option<ApiKey>` + a `None` branch that does not sign--
        // there is no way to build an anonymous client, and `None` cannot be
        // materialized as an empty key: that would send `Authorization:
        // Bearer ` against invariant 2. The `client.rs` change is requested as
        // a patch; until then, fail explicitly with a variant that is NOT
        // `AuthFailed`.
        let Some(secret) = secret else {
            return Err(ProviderInitError::AnonymousCompletionUnsupported {
                provider_id: config.id.clone(),
            });
        };
        let client = OpenAiLlmClient::with_http_and_extra_body(
            http,
            config.base_url.clone(),
            secret,
            config.extra_body.clone(),
        );
        Ok(Self {
            client,
            config,
            proxy_auth_configured,
        })
    }

    /// Provider identifier.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.config.id
    }

    /// Declarative configuration (for diagnostics/snapshots).
    #[must_use]
    pub fn config(&self) -> &ProviderConfig {
        &self.config
    }
}

impl LlmPort for OpenAiCompatibleProvider {
    fn send_completion<'a>(
        &'a self,
        request: crate::domain::llm_port::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = crate::error::Result<crate::domain::llm_port::LlmResponse>,
                > + Send
                + 'a,
        >,
    > {
        let proxy_auth_configured = self.proxy_auth_configured;
        let provider_id = self.config.id.clone();
        Box::pin(async move {
            self.client
                .send_completion(request)
                .await
                .map_err(|e| match &e {
                    // ADR-0021: 407 is a terminal proxy-auth failure. The raw
                    // `ScraperError::Http{407}` is terminal but cannot say whether
                    // a proxy credential was configured — this adds that detail
                    // without ever printing the secret.
                    crate::error::ScraperError::Http { status: 407, .. } => {
                        proxy_407_error(proxy_auth_configured, &provider_id)
                    },
                    _ => e,
                })
        })
    }
}

/// Credential-free 407 diagnostic for the completion path.
fn proxy_407_error(proxy_auth_configured: bool, provider_id: &str) -> crate::error::ScraperError {
    tracing::warn!(
        provider_id = %provider_id,
        status = 407,
        proxy_auth_configured,
        "forward proxy rejected a completion request"
    );
    let detail = if proxy_auth_configured {
        "rejected the configured credential in proxy.auth"
    } else {
        "requires authentication but proxy.auth was not declared"
    };
    crate::error::ScraperError::Extraction(format!(
        "proxy for provider '{provider_id}' returned 407: {detail} (no retry)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::auth_source::AuthSource;
    use crate::domain::llm_port::{ChatMessage, LlmRequest};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config_with(auth: AuthSource) -> ProviderConfig {
        ProviderConfig {
            extra_body: serde_json::Map::new(),
            query_extra_body: serde_json::Map::new(),
            id: "test-provider".to_string(),
            display_name: "Test".to_string(),
            kind: crate::domain::providers::ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.com/v1").expect("url"),
            auth,
            capabilities: vec![crate::domain::providers::Capability::Completion],
            model: Some("gpt-test".to_string()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: crate::domain::network_policy::NetworkPolicy::default(),
            proxy: None,
        }
    }

    /// Key contract: resolve() runs in the CONSTRUCTOR, not on the first call.
    /// A missing credential must fail `with_http`, not `send_completion`.
    #[test]
    fn missing_credential_fails_at_construction_not_first_call() {
        let cfg = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_WIREUP_MISSING_VAR".to_string(),
        });
        let err = match OpenAiCompatibleProvider::new(cfg) {
            Err(e) => e,
            Ok(_) => panic!("must fail at construction"),
        };
        // The message carries provider_id (the AuthError → ProviderInitError
        // conversion contract) and preserves the source.
        assert!(err.to_string().contains("test-provider"), "{err}");
        assert!(matches!(
            err,
            ProviderInitError::AuthFailed {
                source: AuthError::EnvNotSet(_),
                ..
            }
        ));
    }

    /// Full wire-up against wiremock: credential resolved in the constructor,
    /// request signed with the resolved key, via LlmPort.
    #[tokio::test]
    async fn provider_resolves_credential_and_signs_request() {
        let server = MockServer::start().await;
        // The key the provider must sign with (resolved from Env in the ctor).
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("Authorization", "Bearer sk-wireup-test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "pong"}}],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1}
            })))
            .mount(&server)
            .await;

        let env = webfang_test_utils::EnvGuard::with(&[(
            "WEBFANG_TEST_WIREUP_OK_VAR",
            "sk-wireup-test-key",
        )]);
        let cfg = ProviderConfig {
            base_url: url::Url::parse(&server.uri()).expect("url"),
            ..config_with(AuthSource::Env {
                var: "WEBFANG_TEST_WIREUP_OK_VAR".to_string(),
            })
        };
        let provider = match OpenAiCompatibleProvider::new(cfg) {
            Ok(p) => p,
            Err(e) => panic!("wire-up must resolve: {e}"),
        };
        drop(env); // the ctor already resolved: clearing the env has no effect

        let response = match provider
            .send_completion(LlmRequest {
                messages: vec![ChatMessage {
                    role: "user".to_string(),
                    content: "ping".to_string(),
                }],
                model: "gpt-test".to_string(),
                max_tokens: 16,
            })
            .await
        {
            Ok(r) => r,
            Err(e) => panic!("completions OK: {e}"),
        };
        assert_eq!(response.content, "pong");
        assert_eq!(provider.id(), "test-provider");
    }

    /// The shared client is built once and serves N providers; the error
    /// context (provider_id) does not bleed across configs.
    #[test]
    fn shared_http_client_reports_each_provider_context() {
        let client = match build_default_http_client() {
            Ok(c) => c,
            Err(e) => panic!("shared client must build: {e}"),
        };
        let cfg1 = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_SHARED_1".to_string(),
        });
        let mut cfg2 = config_with(AuthSource::Env {
            var: "WEBFANG_TEST_SHARED_2".to_string(),
        });
        cfg2.id = "second-provider".to_string();
        let err1 = match OpenAiCompatibleProvider::with_http(cfg1, client.clone()) {
            Err(e) => e,
            Ok(_) => panic!("missing credential must fail"),
        };
        let err2 = match OpenAiCompatibleProvider::with_http(cfg2, client.clone()) {
            Err(e) => e,
            Ok(_) => panic!("missing credential must fail"),
        };
        assert!(err1.to_string().contains("test-provider"));
        assert!(err2.to_string().contains("second-provider"));
    }
}
