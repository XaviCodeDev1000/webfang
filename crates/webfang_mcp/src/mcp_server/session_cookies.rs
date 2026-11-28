//! The MCP server's cookie-jar scope: `Mcp-Session-Id` → one jar per session.
//!
//! Two responsibilities, both about the same fact (which session a request
//! belongs to):
//!
//! - **Resolution.** The server has no argv for the jar scope, so the value is
//!   read from the environment variable the CLI flag also uses, resolved once at
//!   startup, and logged with the policy for a request that carries no session
//!   id.
//! - **Binding.** A `session`-scoped container is bound per session, lazily, the
//!   first time a request carries a `Mcp-Session-Id`. The binding owns a lease,
//!   so the session's jar is released when the session ends.
//!
//! # Why the session id comes from the HTTP parts
//!
//! rmcp's Streamable HTTP transport consumes the request body and injects the
//! remaining `http::request::Parts` (the `http` crate type rmcp inserts) into the
//! request's extensions, reachable
//! from `RequestContext::extensions`. That is the transport's own, documented
//! seam: the header is not a tool parameter (the pinned inputSchema forbids
//! one) and not a property of the MCP session worker.
//!
//! # A request with no `Mcp-Session-Id`
//!
//! [`NO_SESSION_ID_POLICY`] is `shared`: such a request is fetched with the
//! client's process jar, exactly as before this scope existed. That case is
//! reachable in three ways, and all three are deliberate:
//!
//! - the `initialize` call, which is what CREATES the session — its own header
//!   cannot exist yet;
//! - a stateless transport (`NeverSessionManager`), which never issues ids;
//! - **stdio**, where there is no HTTP request at all. One stdio process serves
//!   one client, so `session` degenerates to "one jar for the process" — which
//!   for a long-lived IDE process means the jar never resets while the IDE is
//!   open. `operation` is the deterministic escape: it bounds the jar to a
//!   single call regardless of transport.
//!
//! The fallback is the historical jar, not a fresh one, because this slice is
//! additive: a transport that never had per-session jars must not gain an
//! unannounced behavior change from a scope that is otherwise opt-in.

use std::sync::Arc;

use rmcp::model::Extensions;
use tokio::sync::OnceCell;

use webfang_core::application::container::Container;
use webfang_core::application::http_client::SessionCookieBinding;
use webfang_core::domain::ports::HttpClientPort;
use webfang_core::domain::JarScope;

/// Environment variable carrying the server-level jar scope.
///
/// The same name the CLI flag uses (`--cookie-jar-scope`), so one exported value
/// configures both surfaces and the two cannot disagree about spelling.
pub(crate) const COOKIE_JAR_SCOPE_ENV: &str = "WEBFANG_COOKIE_JAR_SCOPE";

/// The transport's session header, lowercased as HTTP renders it.
///
/// rmcp keeps its own `HEADER_SESSION_ID` crate-private, and the router's
/// admission cap mirrors this same literal.
pub(crate) const MCP_SESSION_ID_HEADER: &str = "mcp-session-id";

/// What a request that carries no session id is fetched with.
///
/// Pinned by `a_request_without_a_session_id_uses_the_process_jar`: the
/// historical process jar, never a fresh one.
pub(crate) const NO_SESSION_ID_POLICY: &str = "shared";

/// The scope a configured value resolves to, plus the reason a value was
/// rejected (if one was).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScopeResolution {
    /// The scope the server runs with.
    pub(crate) scope: JarScope,
    /// `Some(reason)` when the configured value was not a known scope and the
    /// default was used instead.
    pub(crate) rejected: Option<String>,
}

/// Resolve the configured value into a scope.
///
/// An absent value is the default ([`JarScope::Shared`]) with nothing to report.
/// A value that names no scope keeps the default AND reports why: the server has
/// no argv to fail on, and silently claiming isolation the operator did not get
/// would be worse than the default.
pub(crate) fn resolve_scope(configured: Option<&str>) -> ScopeResolution {
    let Some(raw) = configured else {
        return ScopeResolution {
            scope: JarScope::default(),
            rejected: None,
        };
    };
    match raw.parse::<JarScope>() {
        Ok(scope) => ScopeResolution {
            scope,
            rejected: None,
        },
        Err(reason) => ScopeResolution {
            scope: JarScope::default(),
            rejected: Some(reason),
        },
    }
}

/// Log the effective scope and the no-session-id policy.
///
/// Separate from the resolution so both are testable without touching the
/// process environment.
pub(crate) fn log_effective_scope(resolution: &ScopeResolution) {
    if let Some(reason) = resolution.rejected.as_deref() {
        tracing::warn!(
            env = COOKIE_JAR_SCOPE_ENV,
            reason,
            fallback = %JarScope::default(),
            "mcp_cookie_jar_scope_value_rejected"
        );
    }
    tracing::info!(
        cookie_jar_scope = %resolution.scope,
        no_session_id_falls_back_to = NO_SESSION_ID_POLICY,
        session_key_header = MCP_SESSION_ID_HEADER,
        "mcp_cookie_jar_scope_resolved"
    );
}

/// Apply the configured scope to the container the MCP server will serve from.
///
/// One place for both transports: the HTTP binary and the stdio binary both
/// build their container here, so a scope can never reach one transport and not
/// the other. There is no operator `--cookie` seed on the server: `--cookie` is
/// a CLI scrape input, and the server has no argv for it in this slice.
#[must_use]
pub(crate) fn apply_configured_scope(container: Container) -> Container {
    let resolution = resolve_scope(std::env::var(COOKIE_JAR_SCOPE_ENV).ok().as_deref());
    log_effective_scope(&resolution);
    container.with_cookie_jar_scope(resolution.scope, None)
}

/// The `Mcp-Session-Id` a request carried, when the transport attached one.
///
/// `None` for every non-HTTP transport and for the request that creates the
/// session, which is the documented [`NO_SESSION_ID_POLICY`] case.
fn session_id(extensions: &Extensions) -> Option<String> {
    let parts = extensions.get::<axum::http::request::Parts>()?;
    let value = parts.headers.get(MCP_SESSION_ID_HEADER)?;
    value.to_str().ok().map(str::to_owned)
}

/// The cookie scope of one MCP session, resolved lazily on its first bound call.
///
/// A handler instance serves exactly one MCP session (the transport builds one
/// per session), so the binding is a per-session value: it is created on the
/// first request that names the session and dropped with the handler, which is
/// when the session worker ends.
#[derive(Default)]
pub(crate) struct SessionCookieScope {
    binding: OnceCell<SessionCookieBinding>,
}

impl Clone for SessionCookieScope {
    /// A clone starts UNBOUND.
    ///
    /// A binding is never shared between handlers, because a binding owns the
    /// lease that releases the session's jar: sharing it would let the first
    /// handler to be dropped free the jar of a session the other one is still
    /// serving. The transport builds each session's handler from a pristine
    /// template (`StreamableHttpService`'s service factory), so an unbound clone
    /// is exactly what a new session needs — it binds on its first request.
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl SessionCookieScope {
    /// Bind to the session this request came from, once.
    ///
    /// A request with no session id, or a container that does not keep
    /// per-session jars, leaves the scope unbound — the caller then fetches
    /// through [`Container::http_client`], which is the documented fallback.
    pub(crate) async fn bind(&self, extensions: &Extensions, container: &Container) {
        let Some(session_id) = session_id(extensions) else {
            return;
        };
        let bound = self
            .binding
            .get_or_try_init(|| async { container.bind_session_cookies(&session_id).ok_or(()) })
            .await;
        if let (Ok(_), Some(session)) = (bound, self.bound_session()) {
            // Once per session, at the first call that names it: the operator
            // can tell a real per-session jar from the documented fallback.
            tracing::debug!(
                session,
                session_key_header = MCP_SESSION_ID_HEADER,
                "mcp_session_cookie_scope_bound"
            );
        }
    }

    /// The port this session's fetches must use, or `None` for the container's.
    pub(crate) fn port(&self) -> Option<Arc<dyn HttpClientPort>> {
        self.binding.get().map(SessionCookieBinding::port)
    }

    /// The session this handler is bound to, when it is bound.
    pub(crate) fn bound_session(&self) -> Option<&str> {
        self.binding.get().map(SessionCookieBinding::session)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `MakeWriter` that keeps what the formatter wrote, so a test can read
    /// the log line the operator will see.
    #[derive(Clone, Default)]
    struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Captured {
        fn rendered(&self) -> String {
            let bytes = self
                .0
                .lock()
                .expect("the capture buffer is never poisoned")
                .clone();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("the capture buffer is never poisoned")
                .extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Self;

        fn make_writer(&self) -> Self::Writer {
            self.clone()
        }
    }

    /// Render one startup line, exactly as an operator reads it.
    fn render_startup_line(resolution: &ScopeResolution) -> String {
        // `tracing` caches a callsite's `Interest` process-wide: a callsite first
        // hit with no subscriber stays `never()` forever, and a thread-local
        // `with_default` would then capture nothing. Installing a global sink
        // once forces the callsite to register as always-enabled.
        static WARM: std::sync::Once = std::sync::Once::new();
        WARM.call_once(|| {
            let sink = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish();
            let _ = tracing::subscriber::set_global_default(sink);
        });

        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();
        tracing::subscriber::with_default(subscriber, || log_effective_scope(resolution));
        captured.rendered()
    }

    #[test]
    fn the_startup_line_names_the_scope_and_the_no_session_fallback() {
        let line = render_startup_line(&resolve_scope(Some("session")));

        assert!(
            line.contains("mcp_cookie_jar_scope_resolved"),
            "the startup line must be greppable by name: {line}"
        );
        assert!(
            line.contains("cookie_jar_scope=session"),
            "the effective scope is the one the operator configured: {line}"
        );
        assert!(
            line.contains("no_session_id_falls_back_to=\"shared\""),
            "the no-session-id policy is stated, never guessed: {line}"
        );
        assert!(
            line.contains("session_key_header=\"mcp-session-id\""),
            "the log says which header the scope keys on: {line}"
        );
    }

    #[test]
    fn a_rejected_value_is_reported_on_the_same_startup_line() {
        let line = render_startup_line(&resolve_scope(Some("per-cookie")));

        assert!(
            line.contains("mcp_cookie_jar_scope_value_rejected"),
            "an unusable value must be reported: {line}"
        );
        assert!(
            line.contains("unknown cookie jar scope"),
            "the reason names what was rejected: {line}"
        );
        assert!(
            line.contains("cookie_jar_scope=shared"),
            "the server keeps the default rather than claiming isolation: {line}"
        );
    }

    #[test]
    fn an_absent_value_is_the_default_scope_with_nothing_to_report() {
        let resolution = resolve_scope(None);
        assert_eq!(resolution.scope, JarScope::Shared);
        assert_eq!(resolution.rejected, None);
    }

    #[test]
    fn every_declared_scope_resolves() {
        for (raw, expected) in [
            ("shared", JarScope::Shared),
            ("operation", JarScope::Operation),
            ("session", JarScope::Session),
            ("SESSION", JarScope::Session),
        ] {
            let resolution = resolve_scope(Some(raw));
            assert_eq!(resolution.scope, expected, "value {raw}");
            assert_eq!(resolution.rejected, None, "value {raw}");
        }
    }

    #[test]
    fn an_unknown_value_keeps_the_default_and_names_the_reason() {
        let resolution = resolve_scope(Some("jar-of-jars"));
        assert_eq!(resolution.scope, JarScope::Shared);
        let reason = resolution.rejected.expect("the rejection is reported");
        assert!(reason.contains("unknown cookie jar scope"), "{reason}");
        assert!(
            reason.contains("expected shared, operation or session"),
            "{reason}"
        );
    }

    #[test]
    fn a_request_without_http_parts_has_no_session_id() {
        assert_eq!(session_id(&Extensions::default()), None);
    }

    #[test]
    fn the_session_header_is_what_names_the_session() {
        let mut parts = axum::http::Request::builder()
            .header(MCP_SESSION_ID_HEADER, "session-42")
            .body(())
            .expect("the fixture request builds")
            .into_parts()
            .0;
        parts.headers.insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        );
        let mut extensions = Extensions::default();
        extensions.insert(parts);
        assert_eq!(session_id(&extensions).as_deref(), Some("session-42"));
    }

    #[test]
    fn http_parts_without_the_header_have_no_session_id() {
        let parts = axum::http::Request::builder()
            .body(())
            .expect("builds")
            .into_parts()
            .0;
        let mut extensions = Extensions::default();
        extensions.insert(parts);
        assert_eq!(session_id(&extensions), None);
    }

    #[test]
    fn an_unbound_scope_offers_no_port_of_its_own() {
        let scope = SessionCookieScope::default();
        assert!(scope.port().is_none());
        assert!(scope.bound_session().is_none());
    }
}
