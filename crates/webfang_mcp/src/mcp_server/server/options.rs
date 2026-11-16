//! `ServerOptions` and the auth/bind preflight rules — moved verbatim from the
//! `server.rs` hub (PR 2 of the Ola 1 split, #2584). Re-exported by the hub so
//! every existing `mcp_server::server::…` path is unchanged.

use super::{DEFAULT_MAX_SESSIONS, DEFAULT_SESSION_CAP_WINDOW_SECS};
// rustdoc link target only (doc of ServerOptions::max_sessions):
#[allow(unused_imports)]
use super::MAX_ALLOWED_SESSIONS_CAP;
use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroUsize};

/// Configuration for the MCP HTTP server middleware stack.
///
/// # Redacted `Debug` (#1615, F9)
///
/// [`auth_token`](Self::auth_token) is a raw credential, so this type's
/// `Debug` is hand-written rather than derived: a derived `Debug` would render
/// the token in full, and `ServerOptions` is exactly the kind of value that
/// gets logged on a startup or a config-dump path. The redacted form still
/// prints `"auth_token": Some("[REDACTED]")` — i.e. it says a credential IS
/// configured, which is the fact an operator needs when a 401 surprises
/// them — and never the value.
#[derive(Clone)]
pub struct ServerOptions {
    /// Request timeout in seconds (default: 30).
    pub request_timeout_secs: u64,
    /// Maximum request body size in bytes (default: 10 MB).
    pub body_limit_bytes: usize,
    /// Maximum requests per second (default: 10, matching the `webfang-mcp`
    /// HTTP binary's `--rate` default).
    pub rate_per_second: u32,
    /// Maximum burst size for rate limiting (default: 20, matching the
    /// `webfang-mcp` HTTP binary's `--burst` default).
    pub rate_burst: u32,
    /// Expected Bearer token. When `None`, requests are REFUSED unless
    /// [`Self::allow_anonymous`] is set — see that field.
    pub auth_token: Option<String>,
    /// Serve requests that carry no valid token, with no token configured
    /// (#1611, G-18).
    ///
    /// `false` by default: an unset credential is a refusal, not a mode. The
    /// old default made "bind loopback, configure nothing" — the shipped
    /// configuration — mean "every process that can reach the socket may use
    /// every scraper tool", and that is a fail-open default for a security
    /// boundary. Opting in is one flag, and the HTTP binary prints a warning
    /// when it is used.
    ///
    /// Ignored when [`Self::auth_token`] is set: a configured token is always
    /// required. Anonymous mode is a LOCALHOST development affordance, and
    /// [`require_auth_for_external_bind`] keeps it off any routable bind.
    pub allow_anonymous: bool,
    /// Maximum number of session-creating requests admitted inside one
    /// [`Self::session_cap_window_secs`] window (default: 64, see
    /// [`DEFAULT_MAX_SESSIONS`]). Enforced by the session admission cap; see
    /// `SessionCap` for what "session-creating" means and what the cap does
    /// NOT know.
    ///
    /// A [`NonZeroUsize`], like every count limit in this crate: a cap of zero
    /// is a misconfiguration, not a mode (cf. `McpState::CategoryLimits`).
    /// The magnitude is bounded separately, at the argv boundary, by
    /// [`MAX_ALLOWED_SESSIONS_CAP`].
    pub max_sessions: NonZeroUsize,
    /// Lifetime of one admission slot, in seconds (default: 300, see
    /// [`DEFAULT_SESSION_CAP_WINDOW_SECS`]).
    ///
    /// Also [`NonZeroU64`]: a zero-length window would mean "a slot is never
    /// released", which is a footgun dressed as a mode.
    pub session_cap_window_secs: NonZeroU64,
}

/// Redacted `Debug` for [`ServerOptions`] (#1615, F9).
///
/// Hand-written because the token must never render. The presence marker is
/// the point: `Some("[REDACTED]")` tells an operator that a credential IS set
/// without telling anyone what it is, which is the fact that explains a 401.
impl std::fmt::Debug for ServerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerOptions")
            .field("request_timeout_secs", &self.request_timeout_secs)
            .field("body_limit_bytes", &self.body_limit_bytes)
            .field("rate_per_second", &self.rate_per_second)
            .field("rate_burst", &self.rate_burst)
            .field(
                "auth_token",
                &self.auth_token.as_ref().map(|_| REDACTED_CREDENTIAL),
            )
            .field("allow_anonymous", &self.allow_anonymous)
            .field("max_sessions", &self.max_sessions)
            .field("session_cap_window_secs", &self.session_cap_window_secs)
            .finish()
    }
}

/// Placeholder rendered in place of a credential value (#1615, F9).
///
/// A recognisable constant rather than an empty string: `"Some(\"\")"` in a log
/// reads as "an empty token is configured", which is a different — and wrong —
/// operational conclusion.
pub const REDACTED_CREDENTIAL: &str = "[REDACTED]";

impl Default for ServerOptions {
    fn default() -> Self {
        // The literals are non-zero by construction; the guard is the repo
        // idiom from `McpState::CategoryLimits` (never `expect` in production
        // code). An operator-supplied zero is a different question, and it is
        // answered at the CLI boundary by the fail-fast checks in the
        // `webfang-mcp` binary.
        let nz = |v: usize| {
            NonZeroUsize::new(v).unwrap_or_else(|| unreachable!("limit literal {v} is non-zero"))
        };
        let nz_secs = |v: u64| {
            NonZeroU64::new(v).unwrap_or_else(|| unreachable!("window literal {v} is non-zero"))
        };
        Self {
            request_timeout_secs: 30,
            body_limit_bytes: 10 * 1024 * 1024,
            rate_per_second: 10,
            rate_burst: 20,
            auth_token: None,
            // Fail-closed (#1611, G-18): the shipped default must not be
            // "anyone who can reach the socket".
            allow_anonymous: false,
            max_sessions: nz(DEFAULT_MAX_SESSIONS),
            session_cap_window_secs: nz_secs(DEFAULT_SESSION_CAP_WINDOW_SECS),
        }
    }
}

/// Fail-fast guard for tokenless binds on non-loopback interfaces (REQ-06).
///
/// Binding the MCP server to a routable address without an auth token would
/// expose the scraper surface to the network; the binary must refuse to start
/// before building the container. Loopback binds stay token-free (dev mode)
/// and `IpAddr::is_loopback()` covers both `127.0.0.1` and `::1`.
///
/// # Errors
///
/// Returns a user-facing Spanish error naming the bind address when `bind`
/// is non-loopback and no auth token is present, pointing the operator at
/// `--auth-token` / `WEBFANG_MCP_AUTH_TOKEN`.
///
/// #1611 G-18 extends the guard: with the fail-closed default there is a
/// second way to fail fast, so the two are stated in one place and named
/// together — see [`require_auth_or_explicit_anonymous`], which is what the
/// binary calls. This function remains the non-loopback rule on its own.
pub fn require_auth_for_external_bind(bind: SocketAddr, token_present: bool) -> anyhow::Result<()> {
    if !bind.ip().is_loopback() && !token_present {
        return Err(anyhow::anyhow!(
            "No se puede iniciar el servidor MCP en {bind} sin token de autenticación. Defina --auth-token o WEBFANG_MCP_AUTH_TOKEN."
        ));
    }
    Ok(())
}

/// Fail-fast guard for the fail-closed default (#1611, G-18).
///
/// Three configurations start, and exactly one does not:
///
/// - a token is configured — authenticated operation, any bind;
/// - no token, but the operator explicitly accepted anonymous operation on a
///   LOOPBACK bind — the development mode;
/// - no token and no opt-in, or anonymous operation on a routable bind —
///   refused before the container exists, in Spanish, naming both fixes.
///
/// Failing here rather than at request time is the point: a server that starts
/// and then answers `401` to everything looks like a broken deployment, while
/// this message says which of two knobs is missing.
///
/// # Errors
///
/// Returns a user-facing Spanish error naming the bind address and the two
/// ways to start a token-bearing server.
pub fn require_auth_or_explicit_anonymous(
    bind: SocketAddr,
    token_present: bool,
    allow_anonymous: bool,
) -> anyhow::Result<()> {
    if token_present {
        return Ok(());
    }
    if allow_anonymous {
        // Anonymous mode stays a loopback affordance (REQ-06): the moment the
        // socket is routable, "no credential" is not a development convenience.
        return require_auth_for_external_bind(bind, false);
    }
    Err(anyhow::anyhow!(
        "No se puede iniciar el servidor MCP en {bind} sin token de autenticación ni con --allow-anonymous. \
         Defina --auth-token o WEBFANG_MCP_AUTH_TOKEN, o use --allow-anonymous (o WEBFANG_MCP_ALLOW_ANONYMOUS) \
         solo en loopback."
    ))
}

/// Spanish warning for a routable bind over a plaintext transport (#1611,
/// G-21).
///
/// The transport is plain HTTP with no TLS in this crate, so a non-loopback
/// bind sends `Authorization: Bearer <token>` — the whole scraper surface's
/// credential — in cleartext on every request. That is not a rate-limit or a
/// cap problem: it is a credential on the wire, and no amount of admission
/// control compensates for it. The honest remediation is a TLS-terminating
/// reverse proxy (or a tunnel) in front of the process, which is a deployment
/// decision, so the server's job here is only to say so out loud.
///
/// Loopback binds are not warned about: the traffic never leaves the host.
pub const PLAINTEXT_BIND_WARNING: &str =
    "El transporte MCP es HTTP sin TLS: el token Bearer viaja en claro por la red. Termine TLS delante del servidor (proxy inverso o túnel) antes de exponerlo fuera de localhost.";

/// The plaintext-transport warning for `bind`, or `None` when the bind is
/// loopback and the traffic never leaves the host.
///
/// Returned rather than logged so the rule is unit-testable without capturing
/// tracing output, and so the caller can decide how loudly to say it.
#[must_use]
pub fn plaintext_bind_warning(bind: SocketAddr) -> Option<&'static str> {
    if bind.ip().is_loopback() {
        return None;
    }
    Some(PLAINTEXT_BIND_WARNING)
}
