use super::tests::lan_policy;
use super::*;

use crate::domain::network_policy::NetworkPolicy;
use crate::domain::ssrf_guard::SsrfGuard as _;

/// RFC1918 discard target for the allowlisted-LAN cases. Host and port stay
/// separate literals rather than one `IP:port` constant: that shape reads
/// as `path:line` to tooling scanning evidence locations, and the native
/// review admission layer rejects a reviewer result citing one as
/// `evidence_path_out_of_scope`. Nothing listens on the discard port, so a
/// permitted dial runs out the connect path and a denied one never opens a
/// socket. Composed inline at each use — `tests/ssrf_policy_e2e_test.rs`
/// needs the identical fixture, and a shared helper across the two files
/// would be a jscpd clone against a descending ratchet.
const LAN_DISCARD_HOST: &str = "10.255.255.254";
const LAN_DISCARD_PORT: u16 = 9;

/// Shared port-9 CONNECT proof (#1462): nothing listens there, so
/// resolution must succeed and the failure must be a CONNECT error,
/// never an SSRF rejection.
async fn assert_fails_at_connect_without_ssrf_rejection(client: &wreq::Client) {
    // Port 9 (discard): nothing listens; resolution succeeds and the
    // failure must be a CONNECT error, never an SSRF rejection.
    let err = client
        .get("http://localhost:9/")
        .send()
        .await
        .expect_err("nothing listens on port 9");
    assert!(
        !format!("{err:?}").contains("forbidden address"),
        "permitted dial must fail at CONNECT, not at the guard: {err:?}"
    );
}

// B2 slice 2, layer 4: a redirect hop to a literal the provider's
// policy declares is still FOLLOWED. The discriminator is the terminal
// shape: a stopped hop surfaces the 302 response, a followed one the
// destination's own answer, observed on a listener this test owns.
//
// The follow arm declares 127.0.0.0/8 and redirects to that listener
// instead of dialing the RFC1918 discard target: a dial to
// `10.255.255.254:9` lands on the CI runner's own network stack, whose
// VNET routing decides the outcome (a runner inside 10.0.0.0/8 can
// even answer it) — an environment verdict, not a code one (#2557).
// The allowlist ladder is the same either way
// (`is_forbidden_ip_with_policy`: always-denied → loopback flag →
// allowlist), and the RFC1918 *value* of the pure decision is pinned
// without any socket by `redirect_literal_blocked_honors_the_allowlist`
// in `domain/ssrf_guard.rs`.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn redirect_policy_with_policy_follows_an_allowlisted_literal() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // SOURCE and DESTINATION are both wiremock on 127.0.0.1, which
    // the resolver forbids; the hatch disarms that ONE layer so the
    // dials are served. The redirect policy under test stays fully
    // armed.
    let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "1")]);

    let destination = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&destination)
        .await;
    // Composed at runtime, not written as one `IP:port` literal: that
    // shape reads as `path:line` to tooling that scans evidence
    // locations, and the native review admission layer rejects a
    // reviewer result that cites one as out-of-scope. See the same
    // note in `tests/ssrf_policy_e2e_test.rs`.
    let lan_target = format!("http://{LAN_DISCARD_HOST}:{LAN_DISCARD_PORT}/v1");
    let source = MockServer::start().await;
    // The stop arm keeps the RFC1918 literal: under the default policy
    // it is undeclared, so the policy must stop the hop synchronously
    // — no dial outcome involved.
    Mock::given(method("GET"))
        .and(path("/lan"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", lan_target))
        .mount(&source)
        .await;
    // The follow arm redirects to the loopback listener: that dial's
    // outcome is the code's, never the runner's.
    let followed_target = format!("{}/v1", destination.uri());
    Mock::given(method("GET"))
        .and(path("/follow"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", followed_target))
        .mount(&source)
        .await;

    let guarded: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
        std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);

    let denied = guarded
        .secure_client_with_policy(
            wreq::Client::builder().connect_timeout(std::time::Duration::from_secs(2)),
            false,
            &NetworkPolicy::default(),
        )
        .build()
        .expect("test client must build");
    let stopped = denied
        .get(format!("{}/lan", source.uri()))
        .send()
        .await
        .expect("a stopped hop surfaces the 302 itself");
    assert_eq!(
        stopped.status().as_u16(),
        302,
        "an undeclared LAN literal hop must stop synchronously"
    );

    let allowed = guarded
        .secure_client_with_policy(
            wreq::Client::builder().connect_timeout(std::time::Duration::from_secs(2)),
            false,
            &lan_policy(&["127.0.0.0/8"], &[]),
        )
        .build()
        .expect("test client must build");
    let followed = allowed
        .get(format!("{}/follow", source.uri()))
        .send()
        .await
        .expect("a permitted hop is followed all the way to its destination");
    assert_eq!(
        followed.status().as_u16(),
        200,
        "the allowlisted hop is followed, so the destination answers — \
         the 302 is not the terminal answer"
    );
    let received = destination
        .received_requests()
        .await
        .expect("wiremock logs requests by default");
    assert_eq!(
        received.len(),
        1,
        "exactly the redirected hop reached the destination"
    );
}

// Wiring proof through the trait object: an allowlisted LAN literal
// passes both layers (entry + resolver) and the failure is a
// CONNECT-phase error, never an SSRF rejection.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn secure_client_with_policy_permits_allowlisted_lan_literal() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let guarded: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
        std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);
    let net = lan_policy(&["10.0.0.0/8"], &[]);
    let client = guarded
        .secure_client_with_policy(
            wreq::Client::builder().connect_timeout(std::time::Duration::from_secs(2)),
            false,
            &net,
        )
        .build()
        .expect("test client must build");

    // RFC1918 discard target: nothing listens there, so a PERMITTED
    // dial fails at CONNECT while a denied one fails at the guard.
    let err = client
        .get(format!("http://{LAN_DISCARD_HOST}:{LAN_DISCARD_PORT}/v1"))
        .send()
        .await
        .expect_err("nothing listens on the LAN discard port");
    assert!(
        !format!("{err:?}").contains("ForbiddenResolutionError"),
        "an allowlisted LAN literal must fail at CONNECT, not at the guard: {err:?}"
    );

    // The complement, asserted at the layer that actually owns it: a
    // bare wreq client CANNOT stop an IP literal, because wreq never
    // invokes a custom resolver for one (documented layer boundary
    // in this module's header). Literals are the ENTRY gate's job —
    // which is precisely why the adapter's entry gate had to become
    // policy-aware for the allowlisted-LAN case to exist at all.
    assert!(
        crate::domain::ssrf_guard::is_forbidden_literal_host_with_policy(
            "10.255.255.254",
            false,
            &NetworkPolicy::default()
        ),
        "without the CIDR entry the same literal stays denied at entry"
    );
}

// Wiring proofs: a client built through the guard port
// (`DefaultSsrfGuard::secure_client` — the same construction the
// production sites consume) must enforce at connect time for
// *hostname* targets. `localhost` resolves through getaddrinfo
// (/etc/hosts → 127.0.0.1, ::1) without any network dependency, so
// this stays deterministic in CI.
//
// The guard constructs the resolver per `secure_client` call, and
// that construction reads `DISABLE_VALIDATING_RESOLVER_ENV`; each
// proof therefore holds [`ENV_LOCK`] for its whole body so sibling
// threads cannot race the env mutation (issue #926).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn wired_client_rejects_hostname_resolving_to_loopback() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let client = crate::domain::ssrf_guard::DefaultSsrfGuard
        .secure_client(wreq::Client::builder())
        .build()
        .expect("test client must build");

    let err = client
        .get("http://localhost:9/")
        .send()
        .await
        .expect_err("hostname resolving to loopback must fail at connect");
    assert!(
        format!("{err:?}").contains("ForbiddenResolutionError"),
        "failure must come from the SSRF resolver, not the network: {err:?}"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn wired_client_reaches_connect_when_validation_disabled() {
    let _guard = webfang_test_utils::EnvGuard::with(&[(DISABLE_VALIDATING_RESOLVER_ENV, "1")]);
    let client = crate::domain::ssrf_guard::DefaultSsrfGuard
        .secure_client(wreq::Client::builder())
        .build()
        .expect("test client must build");

    assert_fails_at_connect_without_ssrf_rejection(&client).await;
}

// Unit proof for the guard port: through the `dyn SsrfGuard` trait
// object (the shape production consumers hold), ONE choke-point call
// applies the resolver layer — observable at connect time. The
// redirect-policy layer shares the same impl body.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn secure_client_applies_resolver_through_trait_object() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let guarded: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
        std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);
    let client = guarded
        .secure_client(wreq::Client::builder())
        .build()
        .expect("test client must build");

    let err = client
        .get("http://localhost:9/")
        .send()
        .await
        .expect_err("hostname resolving to loopback must fail at connect");
    assert!(
        format!("{err:?}").contains("ForbiddenResolutionError"),
        "guard trait object must apply the validating resolver: {err:?}"
    );
}

// #1462 (U2): the parameterized variant carries the flag through the
// trait object — a loopback dial succeeds past resolution (and fails
// later at CONNECT, never with an SSRF rejection), with no env hatch
// and no registry mutation.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn secure_client_with_loopback_permits_loopback_dial() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_VALIDATING_RESOLVER_ENV]);
    let guarded: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
        std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);
    let client = guarded
        .secure_client_with_loopback(wreq::Client::builder(), true)
        .build()
        .expect("test client must build");

    assert_fails_at_connect_without_ssrf_rejection(&client).await;
}

// #1462 (U2): redirect-chain proof for the parameterized policy.
// Server A 302-redirects to server B's loopback-literal URL: with the
// flag off the guard stops the redirect synchronously (302 surfaces);
// with the flag on the chain is followed to the loopback target (200).
// IP literals bypass the custom resolver (wreq dials them directly),
// so this needs no env hatch — the policy alone decides.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn redirect_policy_with_loopback_gates_wiremock_chain() {
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let target = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&target)
        .await;
    let source = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", target.uri().as_str()))
        .mount(&source)
        .await;

    let denied = crate::domain::ssrf_guard::DefaultSsrfGuard
        .secure_client_with_loopback(wreq::Client::builder(), false)
        .build()
        .expect("test client must build");
    let stopped = denied
        .get(source.uri())
        .send()
        .await
        .expect("stopped redirect surfaces the 302 response");
    assert_eq!(
        stopped.status().as_u16(),
        302,
        "deny policy must stop the loopback redirect synchronously"
    );

    let allowed = crate::domain::ssrf_guard::DefaultSsrfGuard
        .secure_client_with_loopback(wreq::Client::builder(), true)
        .build()
        .expect("test client must build");
    let body = allowed
        .get(source.uri())
        .send()
        .await
        .expect("permitted redirect must be followed")
        .text()
        .await
        .expect("target body must read");
    assert_eq!(body, "ok");
}
