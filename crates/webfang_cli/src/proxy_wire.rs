//! ADR-0021 `[[proxies]]` catalog loading for the binary (exit 78 on a
//! malformed catalog — never a silent partial list).

use webfang_core::cli::config::{resolve_config_path, ConfigDefaults};
use webfang_core::cli::error::CliExit;
use webfang_core::domain::providers::ProviderConfig;
use webfang_core::domain::proxy_config::ProxyRegistry;
use webfang_core::infrastructure::llm::provider::OpenAiCompatibleProvider;

/// Load the catalog. Duplicate ids are a startup config error.
///
/// # Errors
///
/// [`CliExit::ConfigError`] when two entries share an id.
pub(super) fn load_proxy_registry() -> Result<ProxyRegistry, CliExit> {
    let providers_cfg = ConfigDefaults::load(&resolve_config_path());
    ProxyRegistry::try_new(providers_cfg.proxies).map_err(|e| CliExit::ConfigError(e.to_string()))
}

/// Resolve the `[[proxies]]` catalog and build the completion provider from
/// one already-resolved `ProviderConfig` (the binary's production path).
///
/// # Errors
///
/// A duplicate proxy id or any provider initialization failure, as exit 78.
pub(super) fn build_completion_provider(
    cfg: &ConfigDefaults,
    provider_cfg: &ProviderConfig,
) -> Result<OpenAiCompatibleProvider, CliExit> {
    let registry = ProxyRegistry::try_new(cfg.proxies.clone())
        .map_err(|e| CliExit::ConfigError(e.to_string()))?;
    OpenAiCompatibleProvider::new_with_registry(provider_cfg.clone(), &registry)
        .map_err(|e| CliExit::ConfigError(format!("provider LLM: {e}")))
}
