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

use crate::domain::network_policy::NetworkPolicy;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
