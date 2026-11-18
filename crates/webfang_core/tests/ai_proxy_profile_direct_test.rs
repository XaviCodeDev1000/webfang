#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! ADR-0021 Decision 1/2 pin on the profile DIRECT arm: an ambient
//! `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` must never participate in routing.
//!
//! The CLI reaches this arm only after `build_llm_client` chose a direct
//! provider, so a bug here is latent rather than visible from the binary.
//! This suite builds the guard-level client directly, which is the only way
//! to pin the arm itself.

use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::proxy_config::{ClientSecurityProfile, EndpointPolicy};
use webfang_core::domain::ssrf_guard::{DefaultSsrfGuard, SsrfGuard};
use webfang_test_utils::EnvGuard;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test(flavor = "current_thread")]
async fn profile_direct_arm_ignores_ambient_proxy_env() {
    let destination = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"data":[]}"#, "application/json"))
        .mount(&destination)
        .await;

    let ambient_proxy = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(r#"{"data":[]}"#, "application/json"))
        .mount(&ambient_proxy)
        .await;

    let profile = ClientSecurityProfile::direct(EndpointPolicy {
        allow_loopback: true,
        network_policy: NetworkPolicy::default(),
    });

    let ambient_uri = ambient_proxy.uri();
    // The guard must be alive while the client is built: `wreq` reads the
    // system proxy configuration at build time. It is dropped before the
    // request so no process-wide lock is held across an await.
    let client = {
        let _env = EnvGuard::with(&[
            ("HTTP_PROXY", ambient_uri.as_str()),
            ("HTTPS_PROXY", ambient_uri.as_str()),
            ("ALL_PROXY", ambient_uri.as_str()),
            ("NO_PROXY", ""),
            ("no_proxy", ""),
        ]);
        DefaultSsrfGuard
            .secure_client_with_profile(wreq::Client::builder(), &profile)
            .build()
            .expect("guarded direct-profile client must build")
    };

    let response = client
        .post(format!("{}/v1/embeddings", destination.uri()))
        .send()
        .await
        .expect("direct request must reach the destination");

    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        destination
            .received_requests()
            .await
            .unwrap_or_default()
            .len(),
        1,
        "the destination must receive the request"
    );
    assert!(
        ambient_proxy
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "the ambient proxy must never receive the request"
    );
}
