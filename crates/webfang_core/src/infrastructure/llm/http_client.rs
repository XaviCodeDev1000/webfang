//! Shared SSRF-guarded HTTP client constructor for the OpenAI-compatible
//! transports (ADR-0020 Slice 2, issue #2357).
//!
//! One constructor keeps the Chrome145 emulation and the 60 s request / 10 s
//! connect timeouts identical across the three transports that build their
//! own `wreq::Client`: the completion adapter
//! [`OpenAiLlmClient::new`](crate::infrastructure::llm::client::OpenAiLlmClient),
//! the process-wide shared client
//! [`build_default_http_client`](crate::infrastructure::llm::provider::build_default_http_client)
//! and the remote-embedding adapter's `embedding_http_client`. The policy
//! pair stays port-owned: `None` maps to the plain
//! [`SsrfGuard::secure_client`](crate::domain::ssrf_guard::SsrfGuard::secure_client)
//! shape (public endpoints, loopback denied), `Some(policy)` maps to
//! [`SsrfGuard::secure_client_with_policy`](crate::domain::ssrf_guard::SsrfGuard::secure_client_with_policy)
//! carrying `allow_loopback` and the provider's B2 `network_policy` as
//! per-client parameters — exactly the pre-extraction wiring, unchanged.
//!
//! The builder failure is returned raw: each caller maps it into its own
//! error funnel (`ScraperError::Config` on the completion path vs
//! `ProviderInitError::HttpClient` on the provider paths), preserving
//! today's messages verbatim.

use std::time::Duration;

use crate::domain::auth_source::AuthError;
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::providers::ProviderConfig;
use crate::domain::proxy_config::{
    ClientSecurityProfile, EndpointPolicy, ProxyConfigError, ProxyRegistry,
};

/// Per-request ceiling for AI provider calls. Identical on both ports before
/// this extraction — now defined exactly once.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Connect-phase ceiling: fail fast on a dead or perimeter-filtered endpoint
/// instead of waiting out the full request timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Build one SSRF-guarded `wreq::Client` with the shared transport literals
/// (Chrome145 emulation, 60 s request / 10 s connect).
///
/// # Errors
///
/// Returns the raw `wreq` builder error; callers map it into their own
/// error funnel.
pub(crate) fn build_guarded_http_client(
    allow_loopback: bool,
    network_policy: Option<&NetworkPolicy>,
) -> wreq::Result<wreq::Client> {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Profile::Chrome145)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT);
    let guard = crate::domain::ssrf_guard::ssrf_guard();
    let secured = match network_policy {
        Some(policy) => guard.secure_client_with_policy(builder, allow_loopback, policy),
        None => guard.secure_client(builder),
    };
    secured.build()
}

/// Build one SSRF-guarded client from a full ADR-0021 [`ClientSecurityProfile`].
///
/// The profile separates the destination trust domain (entry gate, redirect
/// stops, direct-dial resolver) from the optional proxy hop's dial policy.
/// `DefaultSsrfGuard::secure_client_with_profile` applies the mandatory order:
/// destination redirect policy → ambient-env neutering → the validated proxy
/// matcher → the proxy-or-destination validating resolver.
///
/// # Errors
///
/// Returns the raw `wreq` builder error; callers map it into their own error
/// funnel, exactly as for [`build_guarded_http_client`].
pub(crate) fn build_profile_http_client(
    profile: &ClientSecurityProfile<'_>,
) -> wreq::Result<wreq::Client> {
    let builder = wreq::Client::builder()
        .emulation(wreq_util::Profile::Chrome145)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT);
    let guard = crate::domain::ssrf_guard::ssrf_guard();
    guard.secure_client_with_profile(builder, profile).build()
}

/// Why a provider's ADR-0021 client could not be built. User-facing messages
/// are Spanish; the variants keep the original causes for diagnostics.
#[derive(Debug, thiserror::Error)]
pub enum LlmClientProfileError {
    /// The provider's proxy declaration was rejected by the domain guard.
    #[error("{0}")]
    Proxy(#[from] ProxyConfigError),
    /// The proxy's declared password source could not be resolved.
    #[error("no se pudo resolver la credencial del proxy: {0}")]
    Auth(#[from] AuthError),
    /// `wreq` refused to build the client.
    #[error("no se pudo construir el cliente HTTP: {0}")]
    Client(String),
}

/// The built client plus the proxy-auth provenance a 407 diagnostic needs.
/// No credential value is carried — only whether one was configured.
pub struct LlmClientBundle {
    /// The guarded client, ready for the adapter.
    pub client: wreq::Client,
    /// Whether the proxy hop was configured with a Basic-auth credential.
    /// `false` also covers a provider with no proxy at all.
    pub proxy_auth_configured: bool,
}

/// Build the HTTP client for one provider, honoring its optional proxy.
///
/// With no `proxy` declaration this is byte-for-byte the ADR-0020 path
/// (destination loopback flag + network policy). With one, the catalog is
/// resolved, the Basic-auth credential is resolved once and installed on the
/// proxy matcher, and the full `ClientSecurityProfile` drives the guard.
///
/// # Errors
///
/// Every [`LlmClientProfileError`] is a startup/config failure: fail loud,
/// never a silent direct-egress fallback.
pub fn build_llm_client(
    config: &ProviderConfig,
    registry: &ProxyRegistry,
) -> Result<LlmClientBundle, LlmClientProfileError> {
    let Some(spec) = config.proxy.as_ref() else {
        tracing::info!(
            provider_id = %config.id,
            proxy_enabled = false,
            "AI transport egress direct"
        );
        return Ok(LlmClientBundle {
            client: build_guarded_http_client(config.allow_loopback, Some(&config.network_policy))
                .map_err(|e| LlmClientProfileError::Client(e.to_string()))?,
            proxy_auth_configured: false,
        });
    };
    let validated = registry.resolve(spec)?;
    tracing::info!(
        provider_id = %config.id,
        proxy_enabled = true,
        proxy_origin = %validated.origin(),
        proxy_id = validated.proxy_id().unwrap_or("inline"),
        "AI transport egress through configured forward proxy"
    );
    let declared_auth = validated.auth().cloned();
    let proxy_auth_configured = declared_auth.is_some();
    let validated = match declared_auth {
        Some(auth) => match auth.password.resolve()? {
            Some(key) => validated.with_basic_auth(&auth.user, &key.expose_secret()),
            None => validated,
        },
        None => validated,
    };
    let profile = ClientSecurityProfile::through_proxy(EndpointPolicy::from(config), &validated);
    Ok(LlmClientBundle {
        client: build_profile_http_client(&profile)
            .map_err(|e| LlmClientProfileError::Client(e.to_string()))?,
        proxy_auth_configured,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::proxy_config::{
        ClientSecurityProfile, EndpointPolicy, ProxyDefinition, ValidatedProxy,
    };

    #[test]
    fn plain_guard_client_builds() {
        assert!(
            build_guarded_http_client(false, None).is_ok(),
            "the plain-guard transport client must construct"
        );
    }

    #[test]
    fn policy_guard_client_builds() {
        let policy = NetworkPolicy::default();
        assert!(
            build_guarded_http_client(false, Some(&policy)).is_ok(),
            "the policy-guard transport client must construct"
        );
    }

    fn direct_policy() -> EndpointPolicy {
        EndpointPolicy {
            allow_loopback: false,
            network_policy: NetworkPolicy::default(),
        }
    }

    fn loopback_proxy() -> ValidatedProxy {
        let definition = ProxyDefinition {
            url: url::Url::parse("http://127.0.0.1:9").expect("valid proxy URL"),
            allow_loopback: true,
            network_policy: NetworkPolicy::default(),
            auth: None,
        };
        ValidatedProxy::try_inline(&definition).expect("loopback proxy must validate")
    }

    #[test]
    fn profile_guard_client_builds() {
        let profile = ClientSecurityProfile::direct(direct_policy());
        assert!(
            build_profile_http_client(&profile).is_ok(),
            "the direct profile client must construct"
        );
    }

    #[test]
    fn proxied_profile_client_builds() {
        let proxy = loopback_proxy();
        let profile = ClientSecurityProfile::through_proxy(direct_policy(), &proxy);
        assert!(
            build_profile_http_client(&profile).is_ok(),
            "the proxied profile client must construct"
        );
    }

    #[test]
    fn proxied_profile_with_resolved_basic_auth_builds() {
        let proxy = loopback_proxy().with_basic_auth("operator", "resolved-secret");
        let profile = ClientSecurityProfile::through_proxy(direct_policy(), &proxy);
        assert!(
            build_profile_http_client(&profile).is_ok(),
            "a credentialed proxy matcher must still build a client"
        );
    }
}
