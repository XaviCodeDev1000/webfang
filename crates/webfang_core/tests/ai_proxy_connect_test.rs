#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::disallowed_methods)
)]

//! ADR-0021 Decision 2 pin on the HTTPS tunnel: a guarded client built with a
//! `through_proxy` profile must open the destination through `CONNECT`.
//!
//! wiremock models HTTP/1.1 forward proxying for `http://` destinations
//! (absolute-form) but not `CONNECT`, so this suite uses the smallest fake
//! that can still fail: a raw loopback listener that records the first
//! request line, answers `200 Connection Established`, and lets the TLS
//! handshake fail. The recorded `CONNECT host:443` line is the builder/wiring
//! evidence the ADR asks for; a client that skipped the tunnel never reaches
//! the listener and the assertion times out.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use webfang_core::domain::network_policy::NetworkPolicy;
use webfang_core::domain::proxy_config::{
    ClientSecurityProfile, EndpointPolicy, ProxyDefinition, ValidatedProxy,
};
use webfang_core::domain::ssrf_guard::{DefaultSsrfGuard, SsrfGuard};

#[tokio::test(flavor = "current_thread")]
async fn profile_proxy_arm_issues_connect_for_https_destination() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("fake proxy binds a loopback port");
    let proxy_addr = listener.local_addr().expect("fake proxy address");
    let (head_tx, head_rx) = oneshot::channel();

    let proxy_task = tokio::spawn(async move {
        let (mut socket, _) = listener
            .accept()
            .await
            .expect("fake proxy accepts one dial");
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while head.last() != Some(&b'\n') {
            let read = socket
                .read(&mut byte)
                .await
                .expect("fake proxy reads the CONNECT line");
            if read == 0 {
                break;
            }
            head.push(byte[0]);
        }
        let head = String::from_utf8_lossy(&head).into_owned();
        socket
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await
            .expect("fake proxy confirms the tunnel");
        socket.flush().await.expect("fake proxy flushes");
        let _ = head_tx.send(head);
    });

    let definition = ProxyDefinition {
        url: format!("http://{proxy_addr}")
            .parse()
            .expect("loopback proxy URL parses"),
        allow_loopback: true,
        network_policy: NetworkPolicy::default(),
        auth: None,
    };
    let proxy = ValidatedProxy::try_inline(&definition).expect("loopback proxy validates");
    let destination = EndpointPolicy {
        allow_loopback: false,
        network_policy: NetworkPolicy::default(),
    };
    let profile = ClientSecurityProfile::through_proxy(destination, &proxy);

    let client = DefaultSsrfGuard
        .secure_client_with_profile(wreq::Client::builder(), &profile)
        .timeout(Duration::from_secs(5))
        .build()
        .expect("guarded proxy-profile client must build");

    let result = client.post("https://ai.invalid/v1/embeddings").send().await;
    assert!(
        result.is_err(),
        "the fake proxy does not speak TLS, so the request must fail after CONNECT"
    );

    let head = tokio::time::timeout(Duration::from_secs(5), head_rx)
        .await
        .expect("the client must dial the configured proxy")
        .expect("the fake proxy must report its CONNECT line");
    assert!(
        head.starts_with("CONNECT ai.invalid:443 HTTP/1.1"),
        "unexpected tunnel request: {head:?}"
    );

    proxy_task.await.expect("fake proxy task joins");
}
