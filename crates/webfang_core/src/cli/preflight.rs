//! Pre-flight configuration and validation helpers.
//!
//! Contains config file merging, HTTP connectivity checks, and display helpers
//! used before the main scraping orchestrator begins.
#![allow(missing_docs)]

use std::path::PathBuf;
use tracing::warn;

use crate::application::crawl_options::CrawlOptions;
use crate::domain::config_value::{ConfigSource, ConfigValue};
use crate::{CliExit, ConcurrencyConfig, ExportFormat, OutputFormat};
use std::collections::BTreeMap;

mod field_book;
pub use field_book::{ArgSources, NormalizedConfig};

mod config_stages;
mod env_cli_stages;
mod js_strategy;
mod normalization;
mod path_resolution;
pub use js_strategy::{check_js_dependencies, resolve_chrome_binary};
pub use normalization::check_adaptive_selectors_feature;
pub use normalization::{merge_budget_overrides, normalize};

/// Preflight: `--elastic` must have at least one wirable vector sink (#695).
///
/// `--elastic` and `--output-vectors` are orthogonal vector destinations
/// (#636): the SQLite sink only exists under the `persistence` feature,
/// while the JSONL stream sink is available in every build. Without
/// `persistence` AND without `--output-vectors`, `--elastic` would wire no
/// sink at all and the run would silently report success with no artifact.
///
/// Fail fast instead: an explicit request the binary cannot honor is a
/// configuration error (exit 78), never a silent no-op.
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--elastic` is
/// enabled, no `--output-vectors` path was given, and the binary was built
/// without the `persistence` feature.
pub fn check_elastic_sink(opts: &CrawlOptions) -> Result<(), CliExit> {
    if !opts.elastic.enabled {
        return Ok(());
    }
    if opts.elastic.output_vectors.is_some() {
        return Ok(());
    }
    if cfg!(feature = "persistence") {
        return Ok(());
    }
    Err(CliExit::ConfigError(
        "--elastic requiere un destino de vectores: este binario fue compilado sin la \
             feature `persistence` (sink SQLite); use --output-vectors <ruta> o un binario \
             compilado con persistencia"
            .into(),
    ))
}

/// Orphan-flag guard (#799): `--db-path` is only consumed by the elastic
/// ingestion pipeline (`build_elastic_ingestion`), so without `--elastic` it is a
/// silent no-op -- the run "succeeds" but persists nothing. Reject it up front
/// with a clear message instead of letting the user lose data unknowingly.
pub fn check_db_path_requires_elastic(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.elastic.db_path.is_some() && !opts.elastic.enabled {
        return Err(CliExit::ConfigError(
            "--db-path solo tiene efecto junto con --elastic; ejecuta con --elastic para \
                 persistir en la base de datos SQLite, o quita --db-path"
                .into(),
        ));
    }
    Ok(())
}

/// Preflight: `--export-format vector` without `--clean-ai` must fail before
/// any network request (#796).
///
/// Mirrors the #703/#652 `output_vectors_gate` in the orchestrator: without
/// `--clean-ai` there are no embeddings, so the vector exporter would write an
/// invalid `export.json` (`dimensions: null`, `model_name: null`, documents
/// without `embeddings`) while still reporting success (exit 0). An explicit
/// request the binary cannot honor is never a silent no-op: it is a
/// data-format error (exit 65). `ExportFormat::Jsonl` (the default) and
/// `ExportFormat::Auto` are never gated.
///
/// # Errors
///
/// Returns [`crate::CliExit::DataFormatError`] (exit 65) when the export
/// format is `Vector` and `--clean-ai` was not given.
///
/// # Why there is no longer a build-capability arm
///
/// This gate used to answer twice: once for a build that carried the local
/// inference stack, once for a build that did not — the second arm told the
/// operator to compile a configuration that no longer exists, an instruction
/// nobody can follow. ADR-0004 paso 6 removed that local stack, so there is
/// exactly ONE question left ("did the operator ask for vectors without
/// asking for the thing that produces them?"), and the sibling gate
/// (`orchestrator::output_vectors_gate`) has already answered it the same
/// way: a data-format error, exit 65.
pub fn check_export_format_vector(opts: &CrawlOptions) -> Result<(), CliExit> {
    if opts.export.export_format != ExportFormat::Vector {
        return Ok(());
    }
    if opts.ai {
        return Ok(());
    }

    warn!("--export-format vector rejected without --clean-ai; no embeddings to export");
    Err(CliExit::DataFormatError(
        "No hay vectores para exportar: '--export-format vector' requiere \
         '--clean-ai' para generar embeddings"
            .to_string(),
    ))
}

// ============================================================================
// LLM extraction provider gate (§8b)
// ============================================================================

/// Preflight: `--extract-with-llm` without a configured `completion` provider
/// must fail at startup, not on the first invocation (`ai-providers-design.md`
/// §8b).
///
/// This is the **expiry of the §8b DEBE**: the Container stays permissive
/// (`llm_port()` is `None` without a provider, by contract), but every service
/// binary that exposes an LLM flag MUST validate the port is present before
/// doing any work. Without this gate a daemon reports OK at boot and then
/// fails when a user or cron invokes the feature, with retries amplifying the
/// failure before anyone notices.
///
/// The check is deliberately **static** (flag + config presence) rather than a
/// probe of the constructed port: a reachable-but-broken provider is a runtime
/// concern with its own error classification, while a missing configuration is
/// a startup configuration error (exit 78).
///
/// # Errors
///
/// Returns [`crate::CliExit::ConfigError`] (exit 78) when `--extract-with-llm`
/// is set but the config declares no provider with the `completion`
/// capability, or when `--llm-provider` names an id that is unknown or lacks
/// the capability.
pub fn check_extract_with_llm(
    opts: &CrawlOptions,
    providers: &crate::domain::providers::ProvidersConfig,
) -> Result<(), CliExit> {
    if !opts.extract_with_llm {
        return Ok(());
    }

    let registry = crate::domain::providers::ProviderRegistry::new(providers.clone());
    let resolution = match opts.llm_provider.as_deref() {
        Some(id) => registry.resolve(id, crate::domain::providers::Capability::Completion),
        None => registry.resolve_default_completion(),
    };

    match resolution {
        Ok(_) => Ok(()),
        Err(e) => {
            warn!(
                requested_provider = ?opts.llm_provider,
                error = %e,
                "--extract-with-llm rejected: no usable completion provider"
            );
            Err(CliExit::ConfigError(format!(
                "'--extract-with-llm' requiere un provider LLM configurado \
                 con la capacidad `completion`: {e}. Configurá un provider \
                 en el archivo de configuración o quitá el flag."
            )))
        },
    }
}

// ============================================================================
// Pre-flight HTTP Connectivity Check (T-070)
// ============================================================================

/// Result of a pre-flight connectivity check.
pub enum PreflightResult {
    /// 2xx or 3xx response — all good
    Ok,
    /// 4xx or 5xx response — connectivity OK but server issue
    Warning(u16),
    /// DNS failure, connection refused, timeout — cannot reach host
    Failed(String),
}

/// Send a HEAD request to verify connectivity before starting discovery.
/// Falls back to GET with Range: bytes=0-0 if HEAD is blocked (405) or times out.
///
/// # SSRF layer 2 (#1615, G-4)
///
/// This path has no caller in the tree today — the discovery flow resolves its
/// seed through guarded code — which is exactly why it was unguarded: nothing
/// exercised it, so nothing noticed it built a client and dialled whatever it
/// was handed. That is a trap, not a defence: the next wiring would inherit an
/// unguarded fetch path, and a HEAD to a cloud-metadata address is a real
/// request.
///
/// Rather than delete a dead-but-public function (a compatibility change the
/// issue explicitly left as a maintainer decision), the guard goes in now, so
/// the function is safe the moment it is wired. The refusal rides the existing
/// `Failed` arm: to a caller asking "can I reach this host?", a refused target
/// and an unreachable one are the same answer, and the reason is carried in
/// the message rather than hidden.
///
/// The GET fallback is covered by the same check because it runs only after
/// this one passed.
pub async fn preflight_check(url: &url::Url) -> PreflightResult {
    if let Err(rejection) = crate::domain::ssrf_guard::reject_forbidden_literal_url(url) {
        return PreflightResult::Failed(rejection.to_string());
    }

    let client = match crate::create_http_client() {
        Ok(c) => c,
        Err(e) => return PreflightResult::Failed(format!("failed to create HTTP client: {e}")),
    };

    match client
        .head(url.as_str())
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(response) => {
            let status = response.status().as_u16();
            if status < 400 {
                PreflightResult::Ok
            } else if status == 405 {
                warn!("HEAD request blocked (405), trying GET fallback...");
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Warning(status)
            }
        },
        Err(e) => {
            if e.is_timeout() || e.is_connect() {
                warn!("HEAD request failed ({}), trying GET fallback...", e);
                preflight_get_fallback(&client, url).await
            } else {
                PreflightResult::Failed(format!("network error: {e}"))
            }
        },
    }
}

/// Fallback to GET with Range: bytes=0-0 when HEAD is blocked.
async fn preflight_get_fallback(client: &wreq::Client, url: &url::Url) -> PreflightResult {
    match client
        .get(url.as_str())
        .header("Range", "bytes=0-0")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => PreflightResult::Ok,
        Ok(resp) => PreflightResult::Warning(resp.status().as_u16()),
        Err(e) => PreflightResult::Failed(format!("HEAD y GET fallaron: {e}")),
    }
}

// ============================================================================
// Display Helpers
// ============================================================================

/// Return emoji or ASCII equivalent based on NO_COLOR setting.
#[inline]
pub fn icon(emoji: &str, ascii: &str) -> String {
    if crate::should_emit_emoji() {
        emoji.to_string()
    } else {
        ascii.to_string()
    }
}

#[cfg(all(test, not(miri)))] // Command::spawn unsupported by Miri (#775)
mod tests_adaptive_selectors;

#[cfg(all(test, not(miri)))]
mod tests_export_format;

#[cfg(all(test, not(miri)))]
mod tests_js_strategy;

#[cfg(all(test, not(miri)))]
mod tests_obscura_binary;

#[cfg(all(test, not(miri)))]
mod tests_obscura_version;

#[cfg(all(test, not(miri)))]
mod tests_path_resolution;

#[cfg(all(test, not(miri)))]
mod tests_sink_gates;

#[cfg(all(test, not(miri)))]
mod tests_version_probe;

// ============================================================================
// Phase 3 — Normalization pipeline tests (split by theme, M3 PR B)
// ============================================================================
#[cfg(test)]
mod normalization_pipeline_tests;

#[cfg(test)]
mod normalization_budget_tests;

#[cfg(test)]
mod normalization_tests_support;
