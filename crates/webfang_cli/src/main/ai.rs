//! AI wiring: adaptive engine, semantic inspector/cleaner, provider loading,
//! remote embedding resolution (Tier 1/Tier 2) and the embedding summary.

use super::*;

/// Build the adaptive selector engine when the feature and flag are both active.
///
/// The `semantic` parameter carries the Tier 2 provider when the run resolved
/// one (an explicit `--tier2-embedding-provider`); `None` degrades the engine
/// to Tier 1 (lexical) repair. `options` is the shared engine configuration —
/// the SAME instance that configured the Tier 2 threshold.
#[cfg(feature = "adaptive-selectors")]
pub(super) fn build_adaptive_engine(
    opts: &CrawlOptions,
    semantic: Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>>,
    options: AdaptiveSelectorOptions,
) -> Option<Arc<AdaptiveSelectorEngine>> {
    if opts.adaptive_selectors {
        tracing::info!(
            semantic_wired = semantic.is_some(),
            "building adaptive selector engine"
        );
        Some(Arc::new(AdaptiveSelectorEngine::new(
            Arc::new(DefaultDomInspector::new()),
            semantic,
            options,
        )))
    } else {
        None
    }
}

/// Build the Tier 2 semantic inspector with an explicit embedding port (ADR-0004).
///
/// The slot holds the provider resolved for the Tier 2 semantic repair — the
/// remote adapter built when `--tier2-embedding-provider` IS set. Without that
/// flag there is no Tier 2 provider at all, and the engine degrades to Tier 1
/// (lexical) repair.
///
/// This is SEPARATE from the vault-search embedding slot (`vault_ports.embedding_port`)
/// which is controlled by `--embedding-provider`. Vault search and Tier 2 repair
/// are distinct privacy domains (issue #1977), and the type system enforces it:
/// the parameter is `Option<Tier2EmbeddingPort>`, and the vault slot's
/// `Option<Arc<dyn EmbeddingPort>>` is not assignable to it.
///
/// Gated on `adaptive-selectors` alone: it is the feature that owns the
/// `AdaptiveSelectorOptions` type, and the only consumer of this function.
#[cfg(feature = "adaptive-selectors")]
pub(super) fn semantic_inspector(
    tier2_port: Option<webfang_core::domain::embedding_port::Tier2EmbeddingPort>,
    options: &AdaptiveSelectorOptions,
) -> Option<Arc<dyn webfang_core::domain::SemanticInspectorPort>> {
    tier2_port.map(|slot| {
        let inspector = webfang_core::infrastructure::scraper::embedding_semantic_inspector::EmbeddingSemanticInspector::new(
            Arc::clone(slot.port()),
            options.semantic_threshold,
        );
        Arc::new(inspector) as Arc<dyn webfang_core::domain::SemanticInspectorPort>
    })
}

/// Build the AI semantic cleaner and the vault-search ports (ADR-0004 paso 6).
///
/// `--clean-ai` carries no local ONNX model: the cleaner is built over
/// whatever `EmbeddingPort` the run resolved — remote only, since ADR-0004
/// removed the local stack. The port is the SAME one the vault slot gets
/// (ONE resolution, ONE probe dial per run), resolved with the pattern that
/// already fills that slot: `--embedding-provider` (flag) >
/// `WEBFANG_EMBEDDING_PROVIDER` (env) > the configured default embedding
/// provider. A `--clean-ai` run with no resolvable remote provider is a
/// `ConfigError` in Spanish naming `--embedding-provider` (exit 78) — never
/// a silent no-op.
///
/// WITHOUT `--clean-ai` this returns no cleaner and the ports the run would
/// have had anyway: today's default build never carried an AI cleaner, and
/// the vault slot is wired only when a provider resolves.
pub(super) fn build_ai_cleaner(
    opts: &CrawlOptions,
    embedding_port: Option<Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>>,
) -> Result<
    (
        Option<Arc<dyn SemanticCleaner>>,
        webfang_core::application::container::VaultAiPorts,
    ),
    CliExit,
> {
    // The chunker has no external dependency: it is a pure Markdown splitter
    // whose only knob is the operator's character budget, so it is wired
    // unconditionally. Without it the vault-search tool bails on a missing
    // chunker even when an embedding provider did resolve.
    let mut ports = webfang_core::application::container::VaultAiPorts {
        text_chunker: Some(Arc::new(
            webfang_core::infrastructure::text_chunker::MarkdownChunker::new(
                opts.ai_config.max_chars,
            ),
        )),
        ..Default::default()
    };
    if let Some(port) = &embedding_port {
        ports.embedding_port = Some(Arc::clone(port));
    }

    if !opts.ai || opts.export.dry_run {
        return Ok((None, ports));
    }

    let Some(port) = embedding_port else {
        return Err(CliExit::ConfigError(
            "--clean-ai requiere un provider de embeddings remoto: pasá \
             '--embedding-provider <id>' (o exportá WEBFANG_EMBEDDING_PROVIDER) \
             con un provider configurado en el archivo de config."
                .to_string(),
        ));
    };

    let cleaner = webfang_core::infrastructure::semantic_cleaner::SemanticCleanerImpl::new(
        port,
        opts.ai_config.max_chars,
        opts.ai_config.threshold,
    );
    let cleaner: Option<Arc<dyn SemanticCleaner>> = Some(Arc::new(cleaner));
    ports.cleaner = cleaner.clone();
    Ok((cleaner, ports))
}

/// Load the provider registry from the config file.
///
/// Shared by the vault and Tier 2 embedding resolvers, which otherwise repeat
/// this prologue verbatim — the duplication ratchet (`scripts/check_duplication.sh`)
/// counts it against a DESCENDING baseline, so it lives here once.
pub(super) fn load_providers() -> webfang_core::domain::providers::ProvidersConfig {
    let providers_cfg = ConfigDefaults::load(&resolve_config_path());
    webfang_core::domain::providers::ProvidersConfig {
        providers: providers_cfg.providers,
    }
}

/// Resolve the embedding port for the whole run (ADR-0004 paso 6).
///
/// An explicit `--embedding-provider` always resolves (fail fast even
/// without `--clean-ai`, so vault search works with no local model); with
/// `--clean-ai` the default slot may also select a remote provider.
/// Otherwise `None` with no config load and no probe — plain runs never dial
/// an embeddings endpoint.
///
/// The port is returned ERASED (`Arc<dyn EmbeddingPort>`), not as the
/// concrete adapter: the `--clean-ai` cleaner is built over the trait object
/// and the vault slot stores the same one, so the endpoint is probed exactly
/// once per run.
pub(super) async fn resolve_remote_embedding(
    opts: &CrawlOptions,
) -> Result<Option<std::sync::Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>>, CliExit>
{
    if opts.embedding_provider.is_none() && !opts.ai {
        return Ok(None);
    }
    let providers = load_providers();
    let proxy_registry = proxy_wire::load_proxy_registry()?;
    webfang_core::cli::llm_wire::build_embedding_provider_with_registry(
        opts,
        &providers,
        opts.ai_config.offline,
        &proxy_registry,
    )
    .await
    .map(|adapter| {
        adapter
            .map(|a| a as std::sync::Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>)
    })
}

/// Resolve the remote embedding adapter for the Tier 2 semantic inspector (ADR-0004).
///
/// Separate from `resolve_remote_embedding` on purpose: vault search and
/// Tier 2 repair are distinct privacy domains (issue #1977). An explicit
/// `--tier2-embedding-provider` always resolves (fail fast even without
/// `--clean-ai`). Without it, Tier 2 has NO provider at all: the local ONNX
/// pool that used to serve this slot is gone (ADR-0004 paso 6), and the
/// vault slot's default embedding provider is never adopted, so a remote
/// provider configured for vault search cannot open a Tier 2 egress (#1981).
/// `None` with no config load and no probe — plain runs never dial an
/// embeddings endpoint for Tier 2.
///
/// Consent check: every resolved Tier 2 provider is remote, so
/// `--tier2-consent` / `WEBFANG_TIER2_CONSENT` MUST be set. A hash of
/// `provider_id|base_url` is logged for auditability.
///
/// The gate is decided inside the builder, at the pure resolution step, so an
/// unauthorized remote slot never reaches the credential or the dial.
#[cfg(feature = "adaptive-selectors")]
pub(super) async fn resolve_tier2_remote_embedding(
    opts: &CrawlOptions,
) -> Result<
    Option<
        std::sync::Arc<webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter>,
    >,
    CliExit,
> {
    // Absent id → the local pool (#1981). Return before loading providers, so
    // the vault-search default can never be adopted here and no config is read.
    if opts.tier2_embedding_provider.is_none() {
        return Ok(None);
    }
    let providers = load_providers();
    // By the time this returns an adapter, the remote endpoint was authorized
    // first and probed second, in that order.
    let adapter = webfang_core::cli::llm_wire::build_tier2_embedding_provider(
        opts.tier2_embedding_provider.as_deref(),
        opts.tier2_consent,
        &providers,
        opts.ai_config.offline,
    )
    .await?;

    // An adapter means the slot resolved to a remote endpoint and consent was
    // granted before the dial.
    if let Some(ref remote) = adapter {
        // Log consent hash for auditability
        let consent_hash = compute_consent_hash(remote.id(), remote.base_url());
        tracing::info!(
            tier2_consent_hash = %consent_hash,
            "Tier 2 remote embeddings consent granted"
        );
    }
    Ok(adapter)
}

/// Compute a short hash for audit logging: sha256(provider_id + "|" + base_url) truncated to 16 chars.
///
/// Gated with its only call site: under default features the caller
/// (`resolve_tier2_remote_embedding`) is compiled out and this would otherwise
/// trip `dead_code` on every non-`adaptive-selectors` build.
#[cfg(feature = "adaptive-selectors")]
pub(super) fn compute_consent_hash(provider_id: &str, base_url: &url::Url) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(provider_id.as_bytes());
    hasher.update(b"|");
    hasher.update(base_url.as_str().as_bytes());
    let hash = hasher.finalize();
    // Manual hex encoding (avoid hex crate dependency)
    hash.iter().map(|b| format!("{b:02x}")).collect::<String>()[..16].to_string()
}

/// Print embedding metrics summary at the end of a successful run.
///
/// Only called when Tier 2 remote embedding adapter was used (explicit
/// `--tier2-embedding-provider`). Reads the accumulated metrics and prints
/// them via `ScrapeSummary` alongside the threshold used.
#[cfg(feature = "adaptive-selectors")]
pub(super) async fn print_embedding_summary(
    remote: &Arc<dyn webfang_core::domain::embedding_port::EmbeddingPort>,
    threshold_used: f32,
) {
    // Downcast to RemoteEmbeddingAdapter to access metrics()
    if let Some(remote_adapter) = remote.as_any().downcast_ref::<webfang_core::infrastructure::llm::remote_embedding::RemoteEmbeddingAdapter>() {
        let metrics = remote_adapter.metrics();
        if metrics.request_count > 0 {
            use webfang_core::cli::summary::ScrapeSummary;
            use std::time::Duration;
            let _summary = ScrapeSummary::new(
                0, 0, 0, 0, 0, 0, Duration::from_secs(0),
                metrics.request_count as usize,
                metrics.prompt_tokens,
                metrics.total_tokens,
                threshold_used,
            );
            // Print only the embedding line (not the full scrape summary)
            let no_color = std::env::var("NO_COLOR").is_ok() || std::env::var("TERM").unwrap_or_default() == "dumb";
            let embedding_line = if no_color {
                format!(
                    "[EMBEDDINGS] {} requests, {} prompt tokens, {} total tokens, threshold={:.2}",
                    metrics.request_count, metrics.prompt_tokens, metrics.total_tokens, threshold_used
                )
            } else {
                format!(
                    "🧠 embeddings: {} requests, {} prompt tokens, {} total tokens, threshold={:.2}",
                    metrics.request_count, metrics.prompt_tokens, metrics.total_tokens, threshold_used
                )
            };
            println!("{embedding_line}");
        }
    }
}
