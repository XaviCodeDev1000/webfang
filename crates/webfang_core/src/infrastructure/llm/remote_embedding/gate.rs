use std::net::{IpAddr, Ipv4Addr};

use crate::domain::providers::ProviderConfig;
use crate::infrastructure::llm::provider::ProviderInitError;

/// Entry gate for the remote endpoint (fetch-chain stage 1): http(s) scheme
/// plus the parameterized, policy-aware literal-IP check.
///
/// Runs at construction so a forbidden `base_url` fails fast at startup,
/// before any socket opens. The literal check MUST be policy-aware: wreq's
/// connector parses IP literals directly and never invokes the custom
/// resolver, so a forbidden-looking LAN literal declared in `allow_cidrs`
/// would otherwise be unreachable no matter what the resolver does.
///
/// Hostnames pass here — they are enforced per connection by the validating
/// resolver, which applies the AND decision (`allow_cidrs` AND `allow_hosts`)
/// on the actual answer set.
///
/// # Errors
///
/// [`ProviderInitError::InvalidBaseUrl`] when the scheme is not http(s) or
/// the host is a forbidden literal. The message names the config field that
/// can open the target, and never points at one that cannot.
pub(super) fn entry_gate(config: &ProviderConfig) -> Result<(), ProviderInitError> {
    let invalid = |msg: String| ProviderInitError::InvalidBaseUrl {
        provider_id: config.id.clone(),
        source: crate::error::ScraperError::Config(msg),
    };
    let scheme = config.base_url.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(invalid(format!(
            "esquema '{scheme}' no permitido para embeddings remotos (solo http/https)"
        )));
    }
    let host = config.base_url.host_str().unwrap_or("");
    if crate::domain::ssrf_guard::is_forbidden_literal_host_with_policy(
        host,
        config.allow_loopback,
        &config.network_policy,
    ) {
        // A literal inside an allowlisted CIDR that is still refused is in the
        // never-allowable set (FIN-017): saying "add it to allow_cidrs" there
        // would send the operator in a circle, because no entry can open it.
        let cidr_hint = config
            .network_policy
            .allows_ip(&host.parse().unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        let hint = if cidr_hint {
            format!(
                "la red de '{host}' no se puede habilitar nunca (link-local, multicast o reservada)"
            )
        } else {
            "para endpoints en LAN declaralo en `network_policy.allow_cidrs` del provider; \
             para endpoints locales activá `allow_loopback` en el archivo de configuración"
                .to_string()
        };
        return Err(invalid(format!(
            "URL de embeddings bloqueada por SSRF: el host '{host}' pertenece a una red interna; {hint}"
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
