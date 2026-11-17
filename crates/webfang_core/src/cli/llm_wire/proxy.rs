//! ADR-0021 completion-provider wiring with the `[[proxies]]` catalog.
//!
//! Split from `llm_wire.rs` so the grandfathered parent keeps its net-zero
//! line budget; the public entry point remains at `llm_wire` via re-export.

use super::*;

/// Same as `build_llm_provider` with the `[[proxies]]` catalog available to
/// resolve a provider's optional proxy declaration.
///
/// # Errors
///
/// A rejected proxy declaration or an unresolved proxy credential (exit 78),
/// in addition to the usual provider-resolution errors.
pub fn build_llm_provider_with_registry(
    opts: &CrawlOptions,
    providers: &ProvidersConfig,
    proxy_registry: &ProxyRegistry,
) -> Result<Option<Arc<OpenAiCompatibleProvider>>, CliExit> {
    if !opts.extract_with_llm {
        return Ok(None);
    }

    let registry = ProviderRegistry::new(providers.clone());
    let resolved: Result<&ProviderConfig, RegistryError> = match opts.llm_provider.as_deref() {
        Some(id) => registry.resolve(id, Capability::Completion),
        None => registry.resolve_default_completion(),
    };
    let config = resolved.map_err(|e| {
        CliExit::ConfigError(format!(
            "'--extract-with-llm' requiere un provider LLM configurado \
             con la capacidad `completion` en el archivo de configuración: {e}"
        ))
    })?;

    match config.kind {
        ProviderKind::OpenAiCompatible => {
            let provider =
                OpenAiCompatibleProvider::new_with_registry(config.clone(), proxy_registry)
                    .map_err(|e| CliExit::ConfigError(e.to_string()))?;
            tracing::info!(
                provider_id = %provider.id(),
                base_url = %provider.config().base_url,
                "LLM extraction provider ready"
            );
            Ok(Some(Arc::new(provider)))
        },
    }
}
