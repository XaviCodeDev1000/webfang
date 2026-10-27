#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

// Temporary probe (not committed): what does wreq produce for unsupported scheme?
#[tokio::test]
async fn probe_scheme_error_kind() {
    let client = wreq::Client::builder().build().expect("client");
    let err = client
        .get("ftp://example.com/x")
        .send()
        .await
        .expect_err("must fail");
    println!("SCHEME-ERR display: {err}");
    println!("  is_builder={} is_request={} is_connect={} is_timeout={} is_decode={} is_redirect={} is_body={}",
        err.is_builder(), err.is_request(), err.is_connect(), err.is_timeout(), err.is_decode(), err.is_redirect(), err.is_body());
    let mut src = std::error::Error::source(&err);
    while let Some(s) = src {
        println!("  source: {s}");
        src = s.source();
    }
    // Also: connect refused (closed port on localhost)
    let err2 = client
        .get("http://127.0.0.1:1/x")
        .send()
        .await
        .expect_err("must fail");
    println!("REFUSED-ERR display: {err2}");
    println!(
        "  is_builder={} is_request={} is_connect={} is_timeout={}",
        err2.is_builder(),
        err2.is_request(),
        err2.is_connect(),
        err2.is_timeout()
    );
}
