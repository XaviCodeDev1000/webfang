//! Shared render blocks for the `show` and `doctor` subcommands (issue #2106).
//!
//! Both commands answer the same operator question — *what does this run
//! actually use?* — and until #2106 they answered it with two different
//! blocks: `doctor` named the resolved embeddings provider, `show` named a
//! **model profile**. The profile layer (`[[model_profiles]]`) describes the
//! LOCAL ONNX models that ADR-0004 step 6 retired, and no run consumes it
//! any more, so presenting `granite-97m` as "the profile of the run" was a
//! false provenance claim (the same class of defect as #1933).
//!
//! The two blocks below are therefore shared verbatim, so `show` and `doctor`
//! can never disagree about the identity of the run:
//!
//! - [`print_provider_block`] — the provider the scrape path would dial.
//! - [`print_historical_profiles`] — the `[[model_profiles]]` metadata,
//!   explicitly labelled as history that no run consumes.
//!
//! The caller owns the section header: the two commands title the same
//! content differently.

use crate::domain::model_profile::{ModelProfileRegistry, ProfileProvenance};
use crate::domain::network_policy::NetworkPolicyMode;
use crate::domain::providers::{ProviderConfig, RedactedUrl};

/// Resolve the character budget BOTH diagnostics render and the run chunks
/// with (S10/T2b). An explicit operator flag wins; otherwise the shipped
/// default profile supplies the budget under the operator's
/// `[[model_profiles]]` overrides. Sharing this helper is what keeps `show`
/// and `doctor` from disagreeing with each other or with the run.
pub(crate) fn governing_budget(
    args: &crate::cli::args::SubcommandArgs,
    profiles: &crate::domain::model_profile::ModelProfilesConfig,
) -> Result<usize, crate::CliExit> {
    let explicit = args.max_chars_explicit.then_some(
        args.max_chars_value
            .unwrap_or(crate::domain::options_spec::ai::DEFAULT_MAX_CHARS),
    );
    crate::domain::model_profile::governed_max_chars(explicit, profiles).map_err(|e| {
        crate::CliExit::ConfigError(format!(
            "could not resolve the governing character budget: {e}"
        ))
    })
}

/// Print the shared provider block: the identity of the run.
///
/// `explicit` tells whether the operator NAMED this provider (through
/// `--embedding-provider` or `WEBFANG_EMBEDDING_PROVIDER`) or whether it is
/// the config-order default. The distinction is the same binary one the repo
/// already uses for `max_chars_explicit` / `threshold_explicit`: a value the
/// operator typed is never presented as a default.
pub fn print_provider_block(provider: &ProviderConfig, explicit: bool) {
    if explicit {
        println!("(origin: operator — --embedding-provider / WEBFANG_EMBEDDING_PROVIDER)");
    } else {
        println!("(origin: config default — first provider with the 'embedding' capability)");
    }

    println!("Configured provider:");
    println!("  id: {}", provider.id);
    println!("  display_name: {}", provider.display_name);
    println!("  kind: {:?}", provider.kind);
    println!("  base_url: {}", RedactedUrl::from_url(&provider.base_url));
    println!(
        "  capabilities: {:?}",
        provider
            .capabilities
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
    );
    match provider.model.as_deref() {
        Some(model) => println!("  model: {model} (declared in the config)"),
        None => println!(
            "  model: NOT declared — the remote endpoint requires it on every request \
             (a remote provider without `model` fails when building the adapter)"
        ),
    }
    // `allow_loopback` is the PROVIDER's own network policy — "may this
    // provider's endpoint live on loopback?" — and NOT the `--offline`
    // guard's semantics, which reject every embeddings provider (loopback
    // included) with exit 78. The label says which of the two it is, so the
    // line cannot be read as "`--offline` allows loopback" (#2166).
    println!(
        "  allow_loopback: {}",
        if provider.allow_loopback {
            "true (provider network policy: allows a loopback endpoint)"
        } else {
            "false (provider network policy: loopback blocked by default)"
        }
    );

    let policy = &provider.network_policy;
    if matches!(policy.mode, NetworkPolicyMode::Allowlist) {
        let cidrs: Vec<String> = policy.allow_cidrs.iter().map(|c| c.to_string()).collect();
        let hosts: Vec<String> = policy.allow_hosts.clone();
        println!(
            "  network_policy: allowlist ({} CIDR(s), {} host(s))",
            cidrs.len(),
            hosts.len()
        );
        for c in &cidrs {
            println!("    allow_cidr: {c}");
        }
        for h in &hosts {
            println!("    allow_host: {h}");
        }
    } else {
        println!("  network_policy: restricted (public endpoints only, with SSRF guard)");
    }

    println!("  auth: {}", provider.auth.label());
    match provider.embedding_dim {
        Some(dim) => println!("  declared dim: {dim}"),
        None => println!(
            "  declared dim: not declared in the config — the endpoint fixes it on each request"
        ),
    }
}

/// Render the shared proxy posture lines (ADR-0021 Slice 3).
///
/// Ambient proxy variables are ALWAYS ignored (#2500). With no configured
/// proxy the two legacy lines are preserved verbatim; with one, the effective
/// origin, provenance, proxy-dial policy and auth-source label are shown.
/// No credential value ever reaches these lines.
pub fn proxy_posture_lines(
    provider: Option<&crate::domain::providers::ProviderConfig>,
    registry: &crate::domain::proxy_config::ProxyRegistry,
) -> Vec<String> {
    let proxy = provider
        .and_then(|p| p.proxy.as_ref())
        .and_then(|spec| registry.resolve(spec).ok());
    let mut lines = vec![
        "Proxy: not used — the HTTP_PROXY/HTTPS_PROXY/ALL_PROXY environment is ignored by design;"
            .to_string(),
    ];
    match proxy {
        None => lines.push(
            "  egress is direct and the SSRF guard always resolves the real destination (#2500)."
                .to_string(),
        ),
        Some(p) => {
            lines.push(format!(
                "  effective proxy: origin '{}' ({}), no credentials in the output",
                p.origin(),
                p.proxy_id()
                    .map(|id| format!("[[proxies]] catalog id '{id}'"))
                    .unwrap_or_else(|| "provider inline table".to_string())
            ));
            let policy = p.endpoint_policy();
            lines.push(format!(
                "  proxy policy: allow_loopback={}, cidrs={}, hosts={}",
                policy.allow_loopback,
                policy.network_policy.allow_cidrs.len(),
                policy.network_policy.allow_hosts.len()
            ));
            lines.push(format!(
                "  proxy authentication: {}",
                p.auth().map(|a| a.password.label()).unwrap_or("anonymous")
            ));
            lines.push(
                "  DNS: destination resolution is delegated to the proxy (no local resolution);\
                 the SSRF guard validates every proxy dial."
                    .to_string(),
            );
        },
    }
    lines
}

/// Print the shared proxy posture line (#2500, ADR-0021 Slice 3).
pub fn print_proxy_posture(
    provider: Option<&crate::domain::providers::ProviderConfig>,
    registry: &crate::domain::proxy_config::ProxyRegistry,
) {
    for line in proxy_posture_lines(provider, registry) {
        println!("{line}");
    }
}

/// Print the shared historical block: the `[[model_profiles]]` metadata.
///
/// Every shipped profile is listed with its provenance and its cited
/// numbers. A profile the operator broke with an invalid override is
/// reported as one line and does NOT abort the listing: a broken historical
/// record must not hide the rest of the diagnostic.
pub fn print_historical_profiles(registry: &ModelProfileRegistry) {
    println!("These profiles are not the identity of the run: no endpoint is dialed from them.");
    println!(
        "The default profile's character budget now governs the cleaner and the completion guard."
    );

    for id in registry.ids() {
        match registry.resolve(id) {
            Ok(profile) => print_historical_profile(&profile),
            Err(e) => println!("profile: {id} (could not resolve: {e})"),
        }
    }

    println!("Note: the character budget above is the one the cleaner enforces when no");
    println!("--max-chars was chosen; the vault dimension is a separate concern.");
}

/// Print one resolved historical profile.
fn print_historical_profile(profile: &crate::domain::model_profile::ModelProfile) {
    let provenance_label = match profile.provenance {
        ProfileProvenance::RepoVerified => "repo-verified",
        ProfileProvenance::OperatorOverride => "operator-override",
    };

    println!("profile: {} ({provenance_label})", profile.id);
    println!("  citation: {}", profile.citation);
    println!("  schema_version: {}", profile.schema_version);
    println!("  dim (native): {}", profile.dim);
    println!(
        "  output_dim (effective in vault): {}",
        profile.output_dim()
    );
    println!("  context: {} tokens", profile.context);
    println!("  chars_per_token: {}", profile.chars_per_token);
    println!(
        "  derived budget (context × chars_per_token): {} characters",
        profile.max_chars()
    );

    if profile.provenance == ProfileProvenance::OperatorOverride {
        println!("  WARNING: the operator overrode the audited identity (dim / matryoshka).");
        println!("  The profile is no longer repo-verified; the citation is still from the");
        println!("  repo for traceability, and the declared value is the operator's.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::auth_source::AuthSource;
    use crate::domain::network_policy::NetworkPolicy;
    use crate::domain::providers::{Capability, ProviderKind};
    use crate::domain::proxy_config::{ProxyAuth, ProxyDefinition, ProxyRegistry, ProxySpec};

    fn provider(proxy: Option<ProxySpec>) -> crate::domain::providers::ProviderConfig {
        crate::domain::providers::ProviderConfig {
            id: "diag".to_string(),
            display_name: "Diag".to_string(),
            kind: ProviderKind::OpenAiCompatible,
            base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
            auth: AuthSource::None,
            capabilities: vec![Capability::Embedding],
            model: Some("m".to_string()),
            embedding_dim: None,
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
            proxy,
        }
    }

    #[test]
    fn direct_posture_keeps_the_2500_wording() {
        let registry = ProxyRegistry::default();
        let lines = proxy_posture_lines(None, &registry);
        assert!(lines[0].contains("ignored by design"));
        assert!(lines.join("\n").contains("egress is direct"));
    }

    fn budget_args(explicit: bool, value: Option<usize>) -> crate::cli::args::SubcommandArgs {
        crate::cli::args::SubcommandArgs {
            provider_id: None,
            max_chars_explicit: explicit,
            max_chars_value: value,
            threshold_explicit: false,
            threshold_value: None,
        }
    }

    fn overridden_budget_profiles() -> crate::domain::model_profile::ModelProfilesConfig {
        crate::domain::model_profile::ModelProfilesConfig {
            profiles: vec![crate::domain::model_profile::ModelProfileOverride {
                id: crate::domain::model_profile::GRANITE_97M_PROFILE_ID.to_string(),
                context: Some(1024),
                chars_per_token: Some(2.0),
                ..Default::default()
            }],
        }
    }

    /// The number `show` prints and the number `doctor` prints must be the
    /// number the run chunks with: all three call the same resolver.
    #[test]
    fn governing_budget_in_diagnostics_comes_from_the_profile() {
        let profiles = overridden_budget_profiles();
        assert_eq!(
            governing_budget(&budget_args(false, None), &profiles).expect("resolves"),
            2048
        );
        // The parsed CLI always carries the (default) value; explicitness is
        // the separate fact that decides whether the profile governs.
        assert_eq!(
            governing_budget(&budget_args(false, Some(98_304)), &profiles).expect("resolves"),
            2048
        );
    }

    #[test]
    fn governing_budget_in_diagnostics_lets_an_explicit_flag_win() {
        assert_eq!(
            governing_budget(&budget_args(true, Some(500)), &overridden_budget_profiles())
                .expect("resolves"),
            500
        );
    }

    #[test]
    fn proxied_posture_names_origin_provenance_and_auth_label_without_secrets() {
        let definition = ProxyDefinition {
            url: url::Url::parse("http://127.0.0.1:8888").expect("url"),
            allow_loopback: true,
            network_policy: NetworkPolicy::default(),
            auth: Some(ProxyAuth {
                user: "operator".to_string(),
                password: AuthSource::Env {
                    var: "WEBFANG_DIAG_PROXY_KEY".to_string(),
                },
            }),
        };
        let config = provider(Some(ProxySpec::Inline(definition)));
        let registry = ProxyRegistry::default();
        let lines = proxy_posture_lines(Some(&config), &registry).join("\n");
        assert!(lines.contains("127.0.0.1:8888"), "origin: {lines}");
        assert!(
            lines.contains("provider inline table"),
            "provenance: {lines}"
        );
        assert!(
            lines.contains("proxy authentication: env"),
            "label: {lines}"
        );
        assert!(lines.contains("DNS:"), "delegation notice: {lines}");
        assert!(!lines.contains("operator"), "user must not leak: {lines}");
        assert!(
            !lines.contains("WEBFANG_DIAG_PROXY_KEY"),
            "var must not leak: {lines}"
        );
    }
}
