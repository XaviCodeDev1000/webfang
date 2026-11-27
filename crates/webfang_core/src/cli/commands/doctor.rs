//! `webfang doctor` — diagnose the full embedding path: config, credential,
//! network reachability, and budget.
//!
//! The three diagnostics the issue asks to keep distinguishable (#1933):
//! 1. **No provider configured** — no embedding-capable provider exists at all.
//! 2. **Credential not resolvable** — the provider declares a credential source
//!    that cannot be satisfied (env var missing, keyring miss, unreadable
//!    encrypted file).
//! 3. **Provider configured but unreachable** — resolves and authenticates, but
//!    the endpoint cannot be probed (DNS, connection, status, parse).
//!
//! Each prints a human-readable block and exits non-zero only when the path is
//! broken (`ConfigError` 78, `NetworkError` 69); a healthy path exits 0.
//!
//! # The provider is the run identity; the profile block is history
//!
//! The provider block IS the identity of the run. The `[[model_profiles]]`
//! metadata is historical — ADR-0004 step 6 retired the local ONNX runtime —
//! and no run consumes it (issue #2106). Reachability is measured by the SAME
//! preflight the scrape path uses ([`llm_wire::build_embedding_provider`]).

use std::path::Path;

use crate::cli::args::SubcommandArgs;
use crate::cli::error::CliExit;
use crate::cli::llm_wire;
use crate::domain::model_profile::{
    ModelProfileRegistry,
    ModelProfilesConfig, // without the profile id (ADR-0004 step 6)
};
use crate::domain::providers::{Capability, ProviderConfig, ProviderRegistry, ProvidersConfig};

/// Run the diagnosis against the providers declared in `config_path`.
///
/// `args` carries the operator's budget knobs and the explicit
/// `--embedding-provider` id, if any. When the id is present it is resolved by
/// id — the same provider the scrape path would dial — and an unknown id is a
/// hard `ConfigError`, never a silent fallback to the config-order default. A
/// diagnostic that reports OK about a provider the run never uses is the worst
/// failure mode for `doctor`.
///
/// The config itself is loaded with the diagnostic policy
/// [`crate::cli::config::load_config_defaults_or_diag`]: a config the run
/// could not load is a hard `ConfigError` naming the file, never a degraded
/// diagnosis about defaults the operator never wrote.
pub async fn run(config_path: &Path, args: &SubcommandArgs) -> CliExit {
    let defaults = match crate::cli::config::load_diagnostic_defaults_or_exit(config_path) {
        Ok(defaults) => defaults,
        Err(exit) => return exit,
    };
    let profiles = defaults.model_profiles.clone();
    let providers = defaults.providers.clone();
    let proxy_registry =
        match crate::domain::proxy_config::ProxyRegistry::try_new(defaults.proxies.clone()) {
            Ok(registry) => registry,
            Err(e) => return CliExit::ConfigError(e.to_string()),
        };
    let registry = ProviderRegistry::new(ProvidersConfig {
        providers: providers.clone(),
    });

    match args.provider_id.as_deref() {
        Some(id) => match registry.resolve(id, Capability::Embedding) {
            Ok(provider) => {
                diagnose_provider(&provider.clone(), args, &profiles, &proxy_registry).await
            },
            Err(e) => {
                println!("\n=== doctor: embeddings path ===\n");
                crate::cli::commands::blocks::print_proxy_posture(None, &proxy_registry);
                println!();
                println!("Status: provider '{id}' could not be resolved");
                println!("  {e}");
                CliExit::ConfigError(format!(
                    "the embeddings provider '{id}' does not exist or does not declare the `embedding` capability: {e}"
                ))
            },
        },
        None => match registry.resolve_default_embedding() {
            Ok(provider) => {
                diagnose_provider(&provider.clone(), args, &profiles, &proxy_registry).await
            },
            Err(_) => diagnose_no_usable_provider(&providers, &proxy_registry),
        },
    }
}

/// No embedding-capable provider resolved. Distinguish "nothing configured"
/// from "something configured, but nothing that can serve embeddings".
fn diagnose_no_usable_provider(
    providers: &[ProviderConfig],
    proxy_registry: &crate::domain::proxy_config::ProxyRegistry,
) -> CliExit {
    println!("\n=== doctor: embeddings path ===\n");
    crate::cli::commands::blocks::print_proxy_posture(None, proxy_registry);
    println!();
    if providers.is_empty() {
        println!("Status: no provider configured");
        println!("  There is no provider in the configuration file.");
        println!("  Connect one with:");
        println!("\n    webfang connect <endpoint url> --model <model>\n");
        println!("  Or declare it manually under [[providers]].\n");
        return CliExit::ConfigError("no provider is configured for embeddings".into());
    }

    let embedding_capable = providers
        .iter()
        .filter(|p| p.has_capability(Capability::Embedding))
        .count();
    if embedding_capable == 0 {
        println!("Status: no embeddings provider");
        println!(
            "  There are {} provider(s) in the file, but none declares the \"embedding\" capability.",
            providers.len()
        );
        println!("  Add `capabilities = [\"embedding\"]` to one of them.\n");
        return CliExit::ConfigError(
            "no provider is configured with the `embedding` capability".into(),
        );
    }

    // There ARE embedding providers, but none resolved as the default remote
    // slot — today that means none is `open_ai_compatible`.
    println!("Status: there are embeddings providers, but none remote by default");
    println!("  The remote slot requires `kind = \"open_ai_compatible\"`. Force one with:");
    println!("\n    webfang --embedding-provider <id>\n");
    CliExit::ConfigError(
        "no remote embeddings provider (open_ai_compatible) is configured by default".into(),
    )
}

/// Diagnose a resolved provider: config, credential, reachability, budget.
async fn diagnose_provider(
    provider: &ProviderConfig,
    args: &SubcommandArgs,
    profiles: &ModelProfilesConfig,
    proxy_registry: &crate::domain::proxy_config::ProxyRegistry,
) -> CliExit {
    println!("\n=== doctor: embeddings path ===\n");
    crate::cli::commands::blocks::print_proxy_posture(Some(provider), proxy_registry);
    println!();
    crate::cli::commands::blocks::print_provider_block(provider, args.provider_id.is_some());
    if let Err(exit) = print_profile_and_budget(args, profiles) {
        return exit;
    }

    // 1. Credential — a hard failure blocks the run before any network hop.
    if let Err(exit) = check_credential(provider) {
        return exit;
    }

    // 2. Reachability — the real preflight, same as the scrape path.
    diagnose_reachability(provider).await
}

/// Resolve the provider's credential and report the outcome.
///
/// A provider that declares a credential it cannot resolve is a broken path
/// (exit 78), reported before any network hop: sending a request with no key
/// would only mask the real cause behind a 401.
fn check_credential(provider: &ProviderConfig) -> Result<(), CliExit> {
    match provider.auth.resolve() {
        Ok(Some(_)) => {
            println!(
                "Credential: resolved (source: {}, value not shown)",
                provider.auth.label()
            );
            Ok(())
        },
        Ok(None) => {
            println!("Credential: none (anonymous endpoint by design)");
            Ok(())
        },
        Err(e) => {
            println!(
                "Credential: NOT resolvable (source: {}) — {e}",
                provider.auth.label()
            );
            Err(CliExit::ConfigError(format!(
                "could not resolve the credential for provider '{}': {e}",
                provider.id
            )))
        },
    }
}

/// Build the real adapter and probe the endpoint, reporting reachability.
async fn diagnose_reachability(provider: &ProviderConfig) -> CliExit {
    // #S14: a target the SSRF guard forbids is decided here, before the
    // adapter, and leaves with 70 instead of the 69 the `ConfigError →
    // NetworkError` remap below would report. The verdict honours the
    // provider's `allow_loopback` and `network_policy`, exactly like the guard
    // the adapter builds.
    if let Some(host) = provider.base_url.host_str() {
        if crate::domain::ssrf_guard::redirect_literal_blocked_with_policy(
            Some(host),
            provider.allow_loopback,
            &provider.network_policy,
        ) {
            println!("Connection: refused by the SSRF guard — forbidden internal host '{host}'");
            return CliExit::SsrfBlocked(format!(
                "the embeddings provider '{}' was refused by the SSRF guard: host '{host}' is a \
                 forbidden internal address",
                provider.id
            ));
        }
    }

    let providers = ProvidersConfig {
        providers: vec![provider.clone()],
    };
    let opts = crate::application::crawl_options::CrawlOptions {
        embedding_provider: Some(provider.id.clone()),
        ..crate::application::crawl_options::CrawlOptions::default()
    };

    match llm_wire::build_embedding_provider(&opts, &providers, false).await {
        Ok(Some(conn)) => {
            println!("Connection: OK — endpoint reachable");
            println!("  Dimension served: {}", conn.embedding_dim());
            CliExit::Success
        },
        Ok(None) => {
            // `Ok(None)` means the slot produced no remote endpoint at all.
            // It used to also mean "the local ONNX pool serves this provider",
            // and that reading is what made `doctor` exit 0 for a backend that
            // no longer existed (ADR-0004 step 6). With the `local_onnx` kind
            // gone, an explicit id that resolves to nothing is a config error.
            println!("Connection: the provider did not resolve as a remote embeddings endpoint");
            CliExit::ConfigError(format!(
                "the provider '{}' is not a remote embeddings endpoint",
                provider.id
            ))
        },
        Err(CliExit::ConfigError(msg)) => {
            println!("Connection: NOT reachable — {msg}");
            CliExit::NetworkError(format!(
                "the embeddings provider '{}' is not reachable: {msg}",
                provider.id
            ))
        },
        Err(other) => other,
    }
}

/// The budget block plus the historical profile metadata.
///
/// The budget block keeps ONLY what the run really honours: `--max-chars`
/// (or `WEBFANG_MAX_CHARS`) and the relevance threshold, each with its
/// origin. The `[[model_profiles]]` numbers are **historical metadata** —
/// they describe the local ONNX models ADR-0004 step 6 retired, and no run
/// consumes them, so they are printed with the profiles and labelled as
/// history (issue #2106).
///
/// Reads the same inputs `show` does — the operator's `[[model_profiles]]`
/// overrides and the argv budget knobs — through the SAME shared resolver, so
/// the two diagnostics cannot contradict each other or the run.
fn print_profile_and_budget(
    args: &SubcommandArgs,
    profiles: &ModelProfilesConfig,
) -> Result<(), CliExit> {
    let max_chars = crate::cli::commands::blocks::governing_budget(args, profiles)?;
    let max_chars_origin = if args.max_chars_explicit {
        "operator"
    } else {
        "profile (default granite-97m)"
    };
    let threshold_origin = if args.threshold_explicit {
        "operator"
    } else {
        "spec default"
    };

    println!("\nCleaner budget:");
    println!("  --max-chars (or WEBFANG_MAX_CHARS): {max_chars} ({max_chars_origin})");
    println!(
        "  relevance threshold: {} ({threshold_origin})\n",
        args.threshold_value.unwrap_or(0.3)
    );

    let registry = ModelProfileRegistry::new(profiles.clone());
    println!("\nHistorical model-profile metadata (ADR-0004 step 4):\n");
    crate::cli::commands::blocks::print_historical_profiles(&registry);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::auth_source::AuthSource;
    use crate::domain::network_policy::NetworkPolicy;
    use crate::domain::providers::ProviderKind;

    fn provider(id: &str, auth: AuthSource, caps: Vec<Capability>) -> ProviderConfig {
        ProviderConfig {
            extra_body: serde_json::Map::new(),
            query_extra_body: serde_json::Map::new(),
            id: id.to_string(),
            display_name: id.to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth,
            capabilities: caps,
            model: Some("nomic-embed-text".to_string()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
            proxy: None,
        }
    }

    #[test]
    fn empty_config_is_a_config_error_with_instructions() {
        let exit = diagnose_no_usable_provider(&[], &Default::default());
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    #[test]
    fn a_provider_without_embedding_capability_is_a_config_error() {
        let p = provider("completion", AuthSource::None, vec![Capability::Completion]);
        let exit = diagnose_no_usable_provider(&[p], &Default::default());
        assert!(matches!(exit, CliExit::ConfigError(_)));
    }

    #[test]
    fn an_unresolvable_credential_is_a_config_error() {
        // `Env` pointing at a variable that is not set: resolution fails.
        //
        // Hermetic on purpose: the contract under test is "the variable is
        // ABSENT, so resolution fails". A shell that exported
        // WEBFANG_TEST_DOCTOR_MISSING would otherwise turn this red with the
        // production code perfectly healthy. `EnvGuard::clean` removes the
        // variable under ENV_LOCK and restores the operator's value on drop,
        // so the test is identical in every environment (issue #926 class).
        let _guard = webfang_test_utils::EnvGuard::clean(&["WEBFANG_TEST_DOCTOR_MISSING"]);
        let p = provider(
            "needs-key",
            AuthSource::Env {
                var: "WEBFANG_TEST_DOCTOR_MISSING".to_string(),
            },
            vec![Capability::Embedding],
        );
        let exit = check_credential(&p);
        assert!(matches!(exit, Err(CliExit::ConfigError(_))));
    }

    #[test]
    fn an_anonymous_provider_resolves_no_credential_without_failing() {
        let p = provider("anon", AuthSource::None, vec![Capability::Embedding]);
        assert!(check_credential(&p).is_ok());
    }

    /// `--embedding-provider <unknown>` must be a hard error naming the id,
    /// never a silent fallback to the config-order default. Regression guard
    /// for the false-green where doctor probed a provider the run never used.
    #[tokio::test]
    async fn an_unknown_explicit_provider_id_is_a_config_error() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[[providers]]\nid = \"known\"\ndisplay_name = \"Known\"\n\
             kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
             auth = { source = \"none\" }\ncapabilities = [\"embedding\"]\n\
             model = \"nomic-embed-text\"\n",
        )
        .expect("write config");

        let exit = run(
            &path,
            &SubcommandArgs {
                provider_id: Some("does-not-exist".into()),
                max_chars_explicit: false,
                max_chars_value: None,
                threshold_explicit: false,
                threshold_value: None,
            },
        )
        .await;
        match exit {
            CliExit::ConfigError(msg) => assert!(
                msg.contains("does-not-exist"),
                "the error must name the unknown id, got: {msg}"
            ),
            other => panic!("expected ConfigError naming the id, got {other:?}"),
        }
    }
}
