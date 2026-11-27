//! Minimal valid provider config for tests across the workspace.
//!
//! Test fixtures use struct update syntax with this value so a new
//! `ProviderConfig` field only needs to be added here, not in every test
//! literal (#3074). This is harness-only: production code has no default
//! provider config.

use webfang_core::domain::auth_source::AuthSource;
use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::providers::{Capability, ProviderConfig, ProviderKind};
use webfang_core::domain::ValidUrl;

/// Return a valid, minimal `open_ai_compatible` provider config.
///
/// Override only the fields a test actually exercises, e.g.
/// `ProviderConfig { id: "p".into(), ..test_provider_config() }`.
///
/// # Panics
///
/// Panics if the hard-coded fixture URL is invalid; it is a compile-time
/// constant, so a panic means the fixture itself was edited incorrectly.
#[must_use]
pub fn test_provider_config() -> ProviderConfig {
    ProviderConfig {
        id: "test-provider".to_string(),
        display_name: "Test Provider".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: ValidUrl::parse("https://api.example.com/v1")
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
        extra_body: Default::default(),
        query_extra_body: Default::default(),
    }
}
