//! Provider configuration and registry for multi-provider AI
//! (`docs/architecture/ai-providers-design.md` §4-5).
//!
//! Domain-only: no I/O, no HTTP, no credential resolution. The config is
//! declarative (serde) and the registry is a concrete lookup struct — the
//! design rejected a `ProviderPlugin` trait and dynamic discovery in favor of
//! a `match` on [`ProviderKind`] in the binary layer.
//!
//! Resolution is **by slot**: a caller names the capability it needs and gets
//! the matching config or a typed error. There is no `AnyProviderHandle`.

use url::Url;

use crate::domain::auth_source::AuthSource;
use crate::domain::config_serde::{deserialize_dim_no_null, deserialize_proxy_spec_no_null};
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::proxy_config::ProxySpec;

#[path = "redacted_url.rs"]
pub(crate) mod redacted_url;
pub use self::redacted_url::{serialize_redacted_url, RedactedUrl, REDACTED_URL_PLACEHOLDER};
/// Versioned model profiles live in the sibling [`crate::domain::model_profile`]
/// module; they are re-exported here because both are read from the same user
/// config file — `[[providers]]` and `[[model_profiles]]` sit next to each
/// other, and the resolver for one has to see the overrides of the other.
pub use crate::domain::model_profile::{
    ModelProfileOverride, ModelProfileRegistry, ModelProfilesConfig, ProfileError,
    ProfileProvenance,
};

/// Kind of provider backend behind a [`ProviderConfig`].
///
/// The binary layer `match`es on this to select the concrete adapter; adding
/// a variant is a compile-time-visible change, never a runtime lookup.
///
/// `local_onnx` used to be the second variant. ADR-0004 paso 6 removed the
/// ONNX pool, so the kind had no adapter left to select — it resolved to
/// nothing while `doctor` still reported it as serving the slot. It was
/// removed in 3.0.0: a config that still declares it now fails to load
/// instead of running without embeddings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// OpenAI-compatible chat/completions API (OpenAI, OpenRouter, Groq,
    /// Together, Ollama, FreeLLM…).
    OpenAiCompatible,
}

/// Capability a provider offers.
///
/// Used by the registry to reject a resolution that asks for something the
/// provider cannot do, instead of letting the failure surface at call time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Can produce embeddings for a text batch.
    Embedding,
    /// Can produce chat completions.
    Completion,
}

/// Error returned by [`ProviderRegistry`] lookups.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// No provider with the requested id exists in the config.
    #[error("provider '{provider_id}' no está configurado")]
    ProviderNotFound {
        /// The id that was requested.
        provider_id: String,
    },
    /// The provider exists but does not offer the requested capability.
    #[error("provider '{provider_id}' no ofrece la capacidad '{capability:?}'")]
    CapabilityMismatch {
        /// The id that was requested.
        provider_id: String,
        /// The capability that was missing.
        capability: Capability,
    },
}

/// A vendor-specific JSON object merged into the request body (#3074).
pub type ExtraBody = serde_json::Map<String, serde_json::Value>;

/// Body keys WebFang owns. An operator may not set them through `extra_body`:
/// doing so could replace `model`, `messages` or credentials after WebFang
/// classified them, bypassing auth and budget checks.
pub const RESERVED_REQUEST_KEYS: &[&str] = &[
    "model",
    "input",
    "messages",
    "response_format",
    "temperature",
    "max_tokens",
    "max_completion_tokens",
    "auth",
    // `send_encoding_format` owns this key: an explicit `extra_body` value
    // would bypass the flag and could request base64 vectors (#3074).
    "encoding_format",
];

/// Default for [`ProviderConfig::send_encoding_format`].
///
/// `false` keeps the request byte-identical to the pre-#3074 payload. Set it
/// to `true` only for gateways that default `encoding_format` to base64.
#[must_use]
pub const fn default_send_encoding_format() -> bool {
    false
}

/// Deserialize an `extra_body` map, rejecting WebFang-owned keys at load time.
///
/// The check is deliberately top level only: reserved keys are protocol
/// critical at the body root, and a nested object (a tool schema, a vendor
/// extension) may legitimately reuse one of those names.
fn deserialize_extra_body<'de, D>(deserializer: D) -> Result<ExtraBody, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let map = <ExtraBody as serde::Deserialize>::deserialize(deserializer)?;
    if let Some(key) = map
        .keys()
        .find(|key| RESERVED_REQUEST_KEYS.contains(&key.as_str()))
    {
        return Err(serde::de::Error::custom(format!(
            "reserved request key '{key}' cannot be set in extra_body"
        )));
    }
    Ok(map)
}

/// Overlay `query_extra_body` on `extra_body`: base keys stay, overlay keys
/// win. Used by vault search so an asymmetric model indexes with `passage`
/// and queries with `query` (#3074).
#[must_use]
pub fn merge_extra_body(base: &ExtraBody, overlay: &ExtraBody) -> ExtraBody {
    let mut merged = base.clone();
    for (key, value) in overlay {
        merged.insert(key.clone(), value.clone());
    }
    merged
}

/// Declarative configuration of one provider (the wizard writes this into the
/// config file).
///
/// Unknown keys are typos, not optional vendor extensions: a key that was
/// silently dropped used to leave operators believing a field was configured
/// (#3074). No field here is `#[serde(flatten)]`, so `deny_unknown_fields` is
/// safe and needs no extra dependency.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// Unique provider identifier (e.g. `"openai"`, `"ollama-local"`).
    pub id: String,
    /// Human-readable name.
    pub display_name: String,
    /// Backend kind — selects the concrete adapter in the binary layer.
    pub kind: ProviderKind,
    /// OpenAI-compatible base URL (e.g. `https://api.openai.com/v1`).
    #[serde(serialize_with = "serialize_redacted_url")]
    pub base_url: Url,
    /// Declared credential source. Resolved once at construction.
    pub auth: AuthSource,
    /// Capabilities this provider offers.
    pub capabilities: Vec<Capability>,
    /// Default chat/completions model for this provider.
    pub model: Option<String>,
    /// Explicit embedding dimension when it differs from the model default.
    ///
    /// `None` (absent) means "adopt the probed dim at startup". An explicit
    /// `null` is a deserialization error, never a silent adopt — a `null` in
    /// the config file is a typo, and typos must fail loudly (#1462).
    #[serde(default, deserialize_with = "deserialize_dim_no_null")]
    pub embedding_dim: Option<usize>,
    /// Permit loopback dials (exactly `127.0.0.1` / `::1`) for this provider.
    ///
    /// Config-file-only opt-in for self-hosted embedding endpoints on the
    /// same machine; there is deliberately NO CLI flag (flags leak into
    /// process lists, and this relaxes an SSRF protection). Absent means
    /// `false`; explicit `null` is a deserialization error. The flag is a
    /// per-client parameter threaded into the guard check — never registry
    /// state (#1462).
    #[serde(default)]
    pub allow_loopback: bool,
    /// Per-provider network allowlist policy (B2, ADR-0004, issue #1947).
    ///
    /// Absent means today's behavior exactly — public endpoints only,
    /// loopback via `allow_loopback`. Entries only take effect with
    /// `mode: "allowlist"` and are validated fail-loud: the never-allowable
    /// ranges (FIN-017) and loopback are rejected at load, never silently
    /// ignored (#1462).
    ///
    /// Threaded into the remote-embedding client as a per-client parameter
    /// (`secure_client_with_policy`), never process state: an allowlisted LAN
    /// literal is reachable with `allow_cidrs` alone, while a LAN *hostname*
    /// additionally requires itself in `allow_hosts` (AND, #1947).
    #[serde(default)]
    pub network_policy: NetworkPolicy,
    /// Explicit forward proxy for this provider's AI egress (ADR-0021):
    /// either an `[[proxies]]` catalog id or an inline table. Absent means
    /// direct egress, exactly today's behavior; an explicit `null` is a
    /// deserialization error, never a silent direct fallback. The proxy dials
    /// under the PROXY's own `allow_loopback`/`network_policy` (a separate
    /// trust domain from this provider's destination policy), and ambient
    /// proxy environment variables stay ignored in every case.
    #[serde(default, deserialize_with = "deserialize_proxy_spec_no_null")]
    pub proxy: Option<ProxySpec>,
    /// Vendor-specific fields merged into `/embeddings` and
    /// `/chat/completions` bodies. Empty means today's payload exactly.
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub extra_body: ExtraBody,
    /// Overlay applied by vault search to `extra_body`, so an asymmetric
    /// embedding model can index with `passage` and query with `query`.
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub query_extra_body: ExtraBody,
    /// Send `encoding_format: "float"` explicitly on `/embeddings`.
    ///
    /// Absent means `false`: the body stays byte-identical to the pre-#3074
    /// payload. Enable it only for a gateway that defaults to base64; NIM and
    /// OpenAI already default to float, and Voyage/Gemini reject the field.
    #[serde(default = "default_send_encoding_format")]
    pub send_encoding_format: bool,
}

impl ProviderConfig {
    /// Whether this provider declares `capability`.
    #[must_use]
    pub fn has_capability(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }
}

/// Minimal valid config for this crate's unit tests.
///
/// Unit tests compile this crate with a different feature set than the
/// `webfang_test_utils` dev-dependency, so its helper is a distinct type here.
/// Integration tests use the one in `webfang_test_utils` instead. Keep BOTH
/// helpers in sync when a `ProviderConfig` field is added or changed.
#[cfg(test)]
pub(crate) fn test_provider_config() -> ProviderConfig {
    ProviderConfig {
        id: "test-provider".to_string(),
        display_name: "Test Provider".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: crate::domain::ValidUrl::parse("https://api.example.com/v1")
            .expect("valid fixture URL")
            .as_url()
            .clone(),
        auth: AuthSource::None,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        embedding_dim: None,
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
        proxy: None,
        extra_body: serde_json::Map::new(),
        query_extra_body: serde_json::Map::new(),
        send_encoding_format: self::default_send_encoding_format(),
    }
}

/// Top-level provider configuration — the `[[providers]]` array of the config
/// file.
///
/// Its sibling in the same file is `[[model_profiles]]`
/// ([`ModelProfilesConfig`]); the two are resolved by separate registries.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    /// Every declared provider, in config order.
    pub providers: Vec<ProviderConfig>,
}

impl ProvidersConfig {
    /// Number of declared providers.
    #[must_use]
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Whether no provider is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Look up a provider by id.
    #[must_use]
    pub fn find(&self, provider_id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == provider_id)
    }
}

/// Lookup struct over [`ProvidersConfig`].
///
/// Resolves **by slot**: the caller names the capability it needs and gets
/// either the provider config or a typed error — never a silent `None` that
/// resurfaces as a late failure.
#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    config: ProvidersConfig,
}

impl ProviderRegistry {
    /// Wrap the parsed config.
    #[must_use]
    pub fn new(config: ProvidersConfig) -> Self {
        Self { config }
    }

    /// The wrapped config.
    #[must_use]
    pub fn config(&self) -> &ProvidersConfig {
        &self.config
    }

    /// Resolve the config of a provider that declares `capability`.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] when the id is unknown,
    /// [`RegistryError::CapabilityMismatch`] when it lacks the capability.
    pub fn resolve(
        &self,
        provider_id: &str,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError> {
        let provider =
            self.config
                .find(provider_id)
                .ok_or_else(|| RegistryError::ProviderNotFound {
                    provider_id: provider_id.to_string(),
                })?;
        if !provider.has_capability(capability) {
            return Err(RegistryError::CapabilityMismatch {
                provider_id: provider_id.to_string(),
                capability,
            });
        }
        Ok(provider)
    }

    /// Resolve the default provider for `capability`: the first provider in
    /// config order that declares it.
    ///
    /// Single choke point behind [`Self::resolve_default_completion`] and
    /// [`Self::resolve_default_embedding`] so the two default slots cannot
    /// drift (#1462).
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] (`<default>`) when no provider
    /// declares `capability`.
    fn resolve_default_by_capability(
        &self,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError> {
        self.config
            .providers
            .iter()
            .find(|p| p.has_capability(capability))
            .ok_or_else(|| RegistryError::ProviderNotFound {
                provider_id: "<default>".to_string(),
            })
    }

    /// Resolve the default completion provider: the first provider in config
    /// order that declares [`Capability::Completion`].
    ///
    /// This is the slot the CLI uses for `--extract-with-llm` when the
    /// operator does not name a provider explicitly.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] when no provider declares
    /// `Completion`.
    pub fn resolve_default_completion(&self) -> Result<&ProviderConfig, RegistryError> {
        self.resolve_default_by_capability(Capability::Completion)
    }

    /// Resolve the default embedding provider: the first provider in config
    /// order that declares [`Capability::Embedding`].
    ///
    /// This is the slot the embedding wiring uses when the operator does not
    /// name a provider explicitly (`--embedding-provider` absent). The kind
    /// match happens in the binary layer: `OpenAiCompatible` builds the remote
    /// adapter and probes it. There is no local arm left to match on —
    /// ADR-0004 paso 6 removed the ONNX pool and the `local_onnx` kind with it.
    ///
    /// # Errors
    ///
    /// [`RegistryError::ProviderNotFound`] (`<default>`) when no provider
    /// declares `Embedding` — the caller reports an empty slot, never a
    /// silent `None` that resurfaces as a late failure.
    pub fn resolve_default_embedding(&self) -> Result<&ProviderConfig, RegistryError> {
        self.resolve_default_by_capability(Capability::Embedding)
    }
}

#[cfg(test)]
#[path = "providers/tests.rs"]
mod tests;
