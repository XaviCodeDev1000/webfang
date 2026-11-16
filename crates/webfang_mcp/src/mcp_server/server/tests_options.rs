//! Tests extraídos del hub `server.rs` por tema (PR 1 de #2584, movimiento puro).
//! Los cuerpos son verbatim del módulo de tests inline original.

use super::{
    plaintext_bind_warning, require_auth_for_external_bind, require_auth_or_explicit_anonymous,
    ServerOptions, DEFAULT_MAX_SESSIONS, DEFAULT_SESSION_CAP_WINDOW_SECS,
};
use crate::mcp_server::McpState;
use std::net::SocketAddr;
use webfang_core::config::Config;
use webfang_core::di::{Container, ContainerExt};

#[cfg_attr(
    miri,
    ignore = "Container::new creates HttpClient with btls-sys FFI (unsupported by Miri)"
)]
#[test]
fn test_server_options_default() {
    let opts = ServerOptions::default();
    assert_eq!(opts.request_timeout_secs, 30);
    assert_eq!(opts.body_limit_bytes, 10 * 1024 * 1024);
    assert_eq!(opts.rate_per_second, 10);
    assert_eq!(opts.rate_burst, 20);
    assert!(opts.auth_token.is_none());
    // #1611 G-18: the shipped default is fail-CLOSED. Pinned here because
    // every integration suite that drives a token-less router has to opt
    // in explicitly, and a flipped default would fail all of them at once
    // — this row is what makes that flip loud rather than mysterious.
    assert!(
        !opts.allow_anonymous,
        "no token configured must not mean anonymous access"
    );
    assert_eq!(opts.max_sessions.get(), DEFAULT_MAX_SESSIONS);
    assert_eq!(
        opts.session_cap_window_secs.get(),
        DEFAULT_SESSION_CAP_WINDOW_SECS
    );
}

// ------------------------------------------------------------------
// F6 session admission cap (#1611)
// ------------------------------------------------------------------

/// The defaults are load-bearing for every other suite: `ServerOptions::default()`
/// is what all the integration tests build, so a default low enough to shed
/// honest traffic would fail the whole crate, and a default high enough to
/// never bind would silently disable the cap. Pinned here, next to the
/// constants, because both are otherwise only observable through a 429.
#[tokio::test]
async fn test_cancel_token_propagates_to_clones() {
    let config = Config::default();
    let container = Container::from_config(config).await.unwrap();
    let state = McpState::new(container);
    let state2 = state.clone();

    // Cancel through one clone, observe via the other.
    state.shutdown_signal();
    assert!(state2.cancel_token.is_cancelled());
}

/// REQ-06: loopback binds stay token-free by default (dev mode).
#[test]
fn require_auth_loopback_no_token_is_ok() {
    let bind: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    assert!(require_auth_for_external_bind(bind, false).is_ok());
}

/// REQ-06: IPv6 loopback (`::1`) counts as loopback.
#[test]
fn require_auth_loopback_v6_no_token_is_ok() {
    let bind: SocketAddr = "[::1]:8080".parse().unwrap();
    assert!(require_auth_for_external_bind(bind, false).is_ok());
}

/// REQ-06: tokenless non-loopback binds fail fast with a Spanish message
/// that names the bind address and the token options.
#[test]
fn require_auth_non_loopback_no_token_is_err() {
    for addr in ["0.0.0.0:8080", "192.168.1.10:8080"] {
        let bind: SocketAddr = addr.parse().unwrap();
        match require_auth_for_external_bind(bind, false) {
            Err(e) => {
                let msg = e.to_string();
                assert!(msg.contains(addr), "message must name the bind: {msg}");
                assert!(
                    msg.contains("token"),
                    "message must mention the token: {msg}"
                );
            },
            Ok(()) => panic!("{addr} must be rejected without a token"),
        }
    }
}

/// REQ-06: a token present lifts the non-loopback restriction.
#[test]
fn require_auth_non_loopback_with_token_is_ok() {
    let bind: SocketAddr = "0.0.0.0:8080".parse().unwrap();
    assert!(require_auth_for_external_bind(bind, true).is_ok());
}

/// #1611 G-18: the fail-closed startup guard, all three cases.
///
/// The third is the regression row for the finding: "no token, no opt-in"
/// used to START, and a server that starts unauthenticated is the whole
/// of the vulnerability. Failing here — before the container exists —
/// turns it into an operator message that names both fixes.
/// #1611 G-21 requires the fail-closed startup guard, and #1611 G-18 makes
/// it symmetric: anonymous operation is loopback-only, and a routable bind
/// additionally gets [`plaintext_bind_warning`] because this transport has
/// no TLS and the bearer token would cross the network in cleartext.
#[test]
fn require_auth_or_explicit_anonymous_covers_all_three_configurations() {
    let loopback: SocketAddr = "127.0.0.1:8080".parse().unwrap();
    let routable: SocketAddr = "0.0.0.0:8080".parse().unwrap();

    // A token always starts, on any bind.
    assert!(require_auth_or_explicit_anonymous(loopback, true, false).is_ok());
    assert!(require_auth_or_explicit_anonymous(routable, true, false).is_ok());
    assert!(require_auth_or_explicit_anonymous(routable, true, true).is_ok());

    // The explicit opt-in is the development mode, and stays local.
    assert!(require_auth_or_explicit_anonymous(loopback, false, true).is_ok());
    assert!(
        require_auth_or_explicit_anonymous(routable, false, true).is_err(),
        "anonymous operation on a routable bind is still refused (REQ-06)"
    );

    // The shipped default: no token and no opt-in does not start.
    match require_auth_or_explicit_anonymous(loopback, false, false) {
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains("127.0.0.1:8080"), "names the bind: {msg}");
            assert!(msg.contains("--auth-token"), "names the token: {msg}");
            assert!(msg.contains("--allow-anonymous"), "names the opt-in: {msg}");
        },
        Ok(()) => panic!("a token-less, opt-in-less bind must be refused"),
    }
}

/// #1611 G-21: a routable bind is warned about, a loopback bind is not —
/// the difference is whether the credential can leave the host at all.
#[test]
fn a_routable_bind_is_warned_about_and_a_loopback_bind_is_not() {
    for addr in ["127.0.0.1:8080", "[::1]:8080", "localhost:1"] {
        let Ok(bind) = addr.parse::<SocketAddr>() else {
            // A name that does not parse as a SocketAddr is not a bind the
            // binary can produce; `localhost` is covered by the two above.
            continue;
        };
        assert_eq!(plaintext_bind_warning(bind), None, "{addr}");
    }
    for addr in ["0.0.0.0:8080", "192.168.1.10:8080"] {
        let bind: SocketAddr = addr.parse().unwrap();
        let warning = plaintext_bind_warning(bind)
            .unwrap_or_else(|| panic!("{addr} must carry the plaintext warning"));
        assert!(
            warning.contains("TLS"),
            "the warning names the fix: {warning}"
        );
    }
}

// ------------------------------------------------------------------
// F2 panic containment (#1611) — the HTTP-layer mapping
// ------------------------------------------------------------------
