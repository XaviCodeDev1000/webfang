//! Explicit per-provider forward proxy for AI egress (ADR-0021).
//!
//! Types + validation only, mirroring [`crate::domain::network_policy`]: the
//! declarative config shapes (`[[proxies]]` catalog, per-provider `proxy`
//! reference or inline table), one fail-closed validator for the proxy URL
//! itself, and the two type-separated security descriptors the guard consumes
//! (`EndpointPolicy` for the destination, `ValidatedProxy` for the proxy
//! dial). Zero ambient environment reads — proxy configuration is
//! config-file-only, exactly like `allow_loopback` (#1462): no CLI URL flag,
//! no userinfo in the URL, no `HTTP_PROXY`/`HTTPS_PROXY` participation.
//!
//! The destination and the proxy are distinct trust domains (ADR-0021
//! Decision 2): a corporate setup targets a public AI endpoint through a
//! private RFC1918 proxy, so allowing the proxy's CIDR for the PROXY dial
//! must never open redirects or destination dials into that CIDR. The type
//! separation here is what keeps that split compile-time rather than
//! convention.
//!
//! Validation is fail-at-startup: everything a `ValidatedProxy` carries is
//! checked once, before any client is built — scheme, credentials, origin
//! shape and the policy-aware literal-IP gate with the FIN-017 never-allowable
//! set still denied even for a proxy.

use std::collections::HashSet;

use url::Url;

use crate::domain::auth_source::AuthSource;
use crate::domain::network_policy::NetworkPolicy;
use crate::domain::providers::ProviderConfig;
use crate::domain::ssrf_guard::is_forbidden_literal_host_with_policy;

/// Error for proxy configuration and validation (ADR-0021). User-facing:
/// Spanish messages name the config field that can fix the problem.
#[derive(Debug, thiserror::Error)]
pub enum ProxyConfigError {
    /// Two `[[proxies]]` entries declare the same `id` — the catalog is
    /// addressed by id, so a duplicate would make the reference ambiguous.
    #[error("id '{id}' duplicado en [[proxies]]; cada entrada del catálogo necesita un id único")]
    DuplicateProxyId {
        /// The duplicated catalog id.
        id: String,
    },
    /// A provider references a `[[proxies]]` id that the catalog does not
    /// declare.
    #[error(
        "el provider referencia el proxy '{proxy_id}' pero ninguna entrada de [[proxies]] lo declara; corregí el id o escribí la tabla inline"
    )]
    UnknownProxyReference {
        /// The referenced-but-undeclared catalog id.
        proxy_id: String,
    },
    /// The proxy URL uses a scheme outside the v1 surface (ADR-0021 Decision
    /// 2: `http`/`https` only — `socks` is not compiled into `wreq`, and a
    /// SOCKS hop would conflate proxy and destination resolution on a single
    /// resolver slot).
    #[error(
        "esquema '{scheme}' no soportado para el proxy '{origin}' (solo proxies forward http:// y https://, con túnel CONNECT para destinos https); socks5/socks5h no están disponibles en esta versión"
    )]
    UnsupportedScheme {
        /// The rejected scheme, as written in the config.
        scheme: String,
        /// The proxy URL, as written in the config.
        origin: String,
    },
    /// The proxy URL carries inline credentials. `url::Url` stores userinfo
    /// in plain, non-zeroized heap strings that leak on `Display`; proxy
    /// authentication composes a plain `user` with an `AuthSource` password
    /// instead (ADR-0021 Decision 1).
    #[error(
        "la URL del proxy '{origin}' lleva credenciales inline (userinfo); declará el usuario y la password en proxy.auth (auth = {{ user = …, password = {{ source = … }} }})"
    )]
    UserInfoNotAllowed {
        /// The offending proxy URL.
        origin: String,
    },
    /// The proxy URL is not a bare origin (it carries path, query or
    /// fragment), which is meaningless for a forward proxy and usually a
    /// config typo.
    #[error(
        "la URL del proxy '{origin}' debe ser un origen (scheme://host[:port]) sin path, query ni fragment"
    )]
    OriginOnly {
        /// The offending proxy URL.
        origin: String,
    },
    /// The proxy host is a forbidden literal IP under the proxy's own policy.
    /// The FIN-017 never-allowable ranges stay denied even when a policy
    /// allowlist would otherwise cover them.
    #[error(
        "el host del proxy '{host}' está prohibido por el guard SSRF (rango never-allowable o literal privado sin opt-in del propio proxy); ajustá allow_loopback / network_policy de la entrada de [[proxies]]"
    )]
    ForbiddenProxyTarget {
        /// The forbidden proxy host.
        host: String,
    },
    /// `wreq` refused to build the proxy matcher from an already-validated
    /// origin — unexpected by construction, surfaced fail-closed.
    #[error("no se pudo construir el matcher del proxy '{origin}': {detail}")]
    ProxyBuild {
        /// The offending proxy URL.
        origin: String,
        /// The `wreq` builder error text.
        detail: String,
    },
}

/// Declarative proxy authentication (Basic): a plain user with the password
/// declared as an [`AuthSource`], resolved once at startup — never inline in
/// the URL (ADR-0021 Decision 1).
///
/// The `zeroize` boundary covers our own `SecretString` only: `wreq`'s
/// `Proxy::basic_auth` keeps the credential in the proxy URI userinfo and a
/// `HeaderValue`, neither of which we control. This type is about config
/// hygiene and `Display` leakage, not end-to-end zeroization.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxyAuth {
    /// Proxy Basic-auth user name.
    pub user: String,
    /// Where the proxy password comes from; resolved once at startup.
    pub password: AuthSource,
}

/// The proxy parameters themselves — shared verbatim by the `[[proxies]]`
/// catalog entries and the per-provider inline table (ADR-0021 Decision 1).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxyDefinition {
    /// Forward-proxy origin, `http`/`https` only; bare origin (no path,
    /// query, fragment), no userinfo.
    pub url: Url,
    /// Permit a loopback proxy dial (exactly `127.0.0.1` / `::1`) — the same
    /// config-file-only opt-in class as the provider's `allow_loopback`,
    /// applied to the PROXY trust domain (a local wiremock/forward-proxy
    /// sidecar).
    #[serde(default)]
    pub allow_loopback: bool,
    /// Proxy-dial network policy: the literal-IP gate and the validating
    /// resolver enforce THIS policy on every proxy socket dial, independent
    /// of the destination's policy.
    #[serde(default)]
    pub network_policy: NetworkPolicy,
    /// Optional Basic-auth credential; absent means an anonymous proxy dial.
    #[serde(default)]
    pub auth: Option<ProxyAuth>,
}

/// One `[[proxies]]` catalog entry: a reusable id plus the proxy definition.
///
/// The `id` is what providers reference (`proxy = "corp-egress"`); the
/// definition is flattened so the TOML entry reads exactly like the inline
/// form minus the id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxyConfig {
    /// Unique catalog identifier — duplicate ids fail at startup.
    pub id: String,
    /// The proxy parameters.
    #[serde(flatten)]
    pub definition: ProxyDefinition,
}

/// What a provider's `proxy` field accepts (ADR-0021 Decision 1): either a
/// string id referencing a `[[proxies]]` entry or an inline table with the
/// proxy definition.
///
/// Untagged on purpose — the config author writes a string or a table, never
/// a wrapper. An explicit `null` never reaches this enum: the field's
/// deserializer rejects it before the untagged matching runs.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ProxySpec {
    /// Reference to a `[[proxies]]` catalog entry by id.
    Named(String),
    /// Inline proxy definition, declared on the provider itself.
    Inline(ProxyDefinition),
}

/// Fail-closed catalog over the `[[proxies]]` entries: duplicate ids are a
/// construction error, and reference resolution validates every proxy before
/// handing it out.
#[derive(Debug, Clone, Default)]
pub struct ProxyRegistry {
    entries: Vec<ProxyConfig>,
}

impl ProxyRegistry {
    /// Build the catalog, rejecting duplicate ids fail-loud.
    ///
    /// # Errors
    ///
    /// [`ProxyConfigError::DuplicateProxyId`] when two entries share an id.
    pub fn try_new(entries: Vec<ProxyConfig>) -> Result<Self, ProxyConfigError> {
        let mut seen = HashSet::with_capacity(entries.len());
        for entry in &entries {
            if !seen.insert(entry.id.as_str()) {
                return Err(ProxyConfigError::DuplicateProxyId {
                    id: entry.id.clone(),
                });
            }
        }
        Ok(Self { entries })
    }

    /// The declared entries, in config order.
    #[must_use]
    pub fn entries(&self) -> &[ProxyConfig] {
        &self.entries
    }

    /// Look up a catalog entry by id.
    #[must_use]
    pub fn find(&self, id: &str) -> Option<&ProxyConfig> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// Resolve a provider's [`ProxySpec`] into a validated proxy.
    ///
    /// `Named` ids must exist in the catalog (fail-loud, never a silent
    /// direct-egress fallback); `Inline` definitions validate in place. Both
    /// paths run the same [`ValidatedProxy`] constructor, so provenance
    /// changes nothing about the guarantees.
    ///
    /// # Errors
    ///
    /// [`ProxyConfigError::UnknownProxyReference`] for an undeclared id,
    /// plus every [`ValidatedProxy`] validation failure.
    pub fn resolve(&self, spec: &ProxySpec) -> Result<ValidatedProxy, ProxyConfigError> {
        match spec {
            ProxySpec::Named(id) => {
                let entry =
                    self.find(id)
                        .ok_or_else(|| ProxyConfigError::UnknownProxyReference {
                            proxy_id: id.clone(),
                        })?;
                ValidatedProxy::try_new(entry)
            },
            ProxySpec::Inline(definition) => ValidatedProxy::try_inline(definition),
        }
    }
}

/// A validated proxy dial descriptor (ADR-0021 Decision 2). Opaque by
/// design: private fields, constructible only through the fail-closed
/// constructors, so no caller can skip a validation step.
///
/// The `wreq` proxy matcher is built HERE, at construction, from the already
/// validated origin — wiring (`SsrfGuard::secure_client_with_profile`) clones
/// it and stays infallible, and every rejection surfaces at startup.
#[derive(Debug, Clone)]
pub struct ValidatedProxy {
    /// Catalog id when the proxy came from a `[[proxies]]` entry; `None` for
    /// the inline form. Observability only (tracing `proxy_id`) — the
    /// guarantees are identical either way.
    id: Option<String>,
    /// Validated origin, `scheme://host[:port]`, no userinfo.
    origin: Url,
    /// The `wreq` matcher built once from the validated origin.
    matcher: wreq::Proxy,
    /// The proxy-dial policy pair (loopback permit + allowlist), carried to
    /// the connect-time resolver and the redirect stop of the PROXY hop.
    policy: NetworkPolicy,
    allow_loopback: bool,
    /// Declared Basic-auth credential, unresolved (startup resolves it).
    auth: Option<ProxyAuth>,
}

impl ValidatedProxy {
    /// Validate a `[[proxies]]` catalog entry. The entry id travels with the
    /// validated proxy for observability.
    ///
    /// # Errors
    ///
    /// Every [`ProxyConfigError`] validation failure for the entry's
    /// definition.
    pub fn try_new(config: &ProxyConfig) -> Result<Self, ProxyConfigError> {
        Self::build(&config.definition, Some(config.id.clone()))
    }

    /// Validate an inline proxy definition (provider-declared, no catalog
    /// id).
    ///
    /// # Errors
    ///
    /// Every [`ProxyConfigError`] validation failure for the definition.
    pub fn try_inline(definition: &ProxyDefinition) -> Result<Self, ProxyConfigError> {
        Self::build(definition, None)
    }

    /// The single validation path — both constructors funnel through here so
    /// the checks cannot drift between the named and inline forms.
    fn build(definition: &ProxyDefinition, id: Option<String>) -> Result<Self, ProxyConfigError> {
        let origin = &definition.url;
        let scheme = origin.scheme();
        if scheme != "http" && scheme != "https" {
            return Err(ProxyConfigError::UnsupportedScheme {
                scheme: scheme.to_string(),
                origin: origin.to_string(),
            });
        }
        if !origin.username().is_empty() || origin.password().is_some() {
            return Err(ProxyConfigError::UserInfoNotAllowed {
                origin: origin.to_string(),
            });
        }
        let path = origin.path();
        if !(path.is_empty() || path == "/")
            || origin.query().is_some()
            || origin.fragment().is_some()
        {
            return Err(ProxyConfigError::OriginOnly {
                origin: origin.to_string(),
            });
        }
        let host = origin.host_str().unwrap_or_default();
        if is_forbidden_literal_host_with_policy(
            host,
            definition.allow_loopback,
            &definition.network_policy,
        ) {
            return Err(ProxyConfigError::ForbiddenProxyTarget {
                host: host.to_string(),
            });
        }
        let matcher =
            wreq::Proxy::all(origin.as_str()).map_err(|e| ProxyConfigError::ProxyBuild {
                origin: origin.to_string(),
                detail: e.to_string(),
            })?;
        Ok(Self {
            id,
            origin: origin.clone(),
            matcher,
            policy: definition.network_policy.clone(),
            allow_loopback: definition.allow_loopback,
            auth: definition.auth.clone(),
        })
    }

    /// The catalog id, when the proxy came from `[[proxies]]`.
    #[must_use]
    pub fn proxy_id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// The validated origin, `scheme://host[:port]` — never userinfo (the
    /// constructors reject it before this point).
    #[must_use]
    pub fn origin(&self) -> &Url {
        &self.origin
    }

    /// The prebuilt `wreq` matcher; wiring clones it into the client builder.
    #[must_use]
    pub fn wreq_proxy(&self) -> &wreq::Proxy {
        &self.matcher
    }

    /// The declared Basic-auth credential, unresolved.
    #[must_use]
    pub fn auth(&self) -> Option<&ProxyAuth> {
        self.auth.as_ref()
    }

    /// The proxy-dial policy pair: what the connect-time resolver and the
    /// proxy-hop redirect stop enforce on every dial to this proxy.
    #[must_use]
    pub fn endpoint_policy(&self) -> EndpointPolicy {
        EndpointPolicy {
            allow_loopback: self.allow_loopback,
            network_policy: self.policy.clone(),
        }
    }
}

/// The security descriptor governing a DESTINATION dial (provider `base_url`):
/// entry-gate literal check, redirect-policy literal stops, and — without a
/// proxy — the connect-time resolver (ADR-0021 Decision 2).
///
/// Deliberately a distinct type from `ValidatedProxy`: the destination and
/// the proxy are separate trust domains, and sharing one descriptor would let
/// an allowlisted proxy CIDR open destination redirects into that CIDR.
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    /// Destination loopback permit (the provider's `allow_loopback`).
    pub allow_loopback: bool,
    /// Destination network policy (the provider's `network_policy`).
    pub network_policy: NetworkPolicy,
}

impl From<&ProviderConfig> for EndpointPolicy {
    fn from(provider: &ProviderConfig) -> Self {
        Self {
            allow_loopback: provider.allow_loopback,
            network_policy: provider.network_policy.clone(),
        }
    }
}

/// One client's complete security posture: where the request is going and
/// whether it goes through a proxy (ADR-0021 Decision 2).
///
/// `via_proxy: None` means direct egress — byte-for-byte today's behavior.
#[derive(Debug, Clone)]
pub struct ClientSecurityProfile<'a> {
    /// The destination trust domain.
    pub destination: EndpointPolicy,
    /// The validated proxy hop, when configured.
    pub via_proxy: Option<&'a ValidatedProxy>,
}

impl<'a> ClientSecurityProfile<'a> {
    /// Direct egress profile: destination policy only, no proxy hop.
    #[must_use]
    pub fn direct(destination: EndpointPolicy) -> Self {
        Self {
            destination,
            via_proxy: None,
        }
    }

    /// Proxied profile: destination policy governs destination/redirect
    /// checks, the proxy's own policy governs the proxy dial.
    #[must_use]
    pub fn through_proxy(destination: EndpointPolicy, proxy: &'a ValidatedProxy) -> Self {
        Self {
            destination,
            via_proxy: Some(proxy),
        }
    }
}

#[cfg(test)]
mod tests;
