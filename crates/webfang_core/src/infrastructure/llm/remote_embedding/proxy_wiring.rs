//! ADR-0021 constructors for `RemoteEmbeddingAdapter` that resolve the
//! provider's optional proxy before building the guarded client.
//!
//! Inherent impls may live in any module of the same crate; keeping these here
//! preserves the grandfathered parent file's net-zero line budget while the
//! domain guard chain stays in `http_client.rs` and `infrastructure/ssrf.rs`.

use super::*;
use crate::domain::proxy_config::ProxyRegistry;
use crate::infrastructure::llm::http_client::build_llm_client;

impl RemoteEmbeddingAdapter {
    /// Build the adapter resolving the credential ONCE (startup).
    ///
    /// The entry gate runs first (forbidden `base_url` fails here), then the
    /// credential resolves, then the parameterized guarded client is built.
    ///
    /// # Errors
    ///
    /// [`ProviderInitError`] when the entry gate rejects the URL, the
    /// credential does not resolve, the model is undeclared, or the shared
    /// client cannot be built.
    pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
        Self::new_with_registry(config, &ProxyRegistry::default())
    }

    /// Build with the provider's proxy resolved against `registry`.
    ///
    /// # Errors
    ///
    /// Any [`ProviderInitError`] from the proxy declaration or `with_http`.
    pub fn new_with_registry(
        config: ProviderConfig,
        registry: &ProxyRegistry,
    ) -> Result<Self, ProviderInitError> {
        let bundle = build_llm_client(&config, registry)
            .map_err(|e| ProviderInitError::HttpClient(e.to_string()))?;
        Self::with_http_and_proxy_auth(config, bundle.client, bundle.proxy_auth_configured)
    }

    /// Records proxy-auth provenance for the 407 diagnostic; never a secret.
    ///
    /// # Errors
    ///
    /// Same as [`Self::with_http`](super::RemoteEmbeddingAdapter::with_http).
    pub fn with_http_and_proxy_auth(
        config: ProviderConfig,
        http: wreq::Client,
        proxy_auth_configured: bool,
    ) -> Result<Self, ProviderInitError> {
        super::entry_gate(&config)?;
        let auth_source = config.auth.label();
        let secret = config
            .auth
            .resolve()
            .map_err(|source| ProviderInitError::AuthFailed {
                provider_id: config.id.clone(),
                source,
            })?;
        let model = config
            .model
            .clone()
            .ok_or_else(|| ProviderInitError::InvalidBaseUrl {
                provider_id: config.id.clone(),
                source: crate::error::ScraperError::Config(format!(
                    "provider '{}' does not declare `model`: the remote endpoint requires it on every request",
                    config.id
                )),
            })?;
        Ok(Self {
            http,
            base_url: config.base_url.clone(),
            model,
            api_key: secret,
            auth_source,
            proxy_auth_configured,
            provider_id: config.id.clone(),
            extra_body: config.extra_body.clone(),
            query_extra_body: config.query_extra_body.clone(),
            pinned_dim: config.embedding_dim,
            dim: OnceLock::new(),
            usage_prompt_tokens: std::sync::atomic::AtomicU64::new(0),
            usage_total_tokens: std::sync::atomic::AtomicU64::new(0),
            usage_request_count: std::sync::atomic::AtomicU64::new(0),
            dimension_rejections: std::sync::atomic::AtomicU64::new(0),
        })
    }
}
