//! Shared fetch-and-parse arrange for the sitemap unit tests (#1889, T7
//! slice-14): serves each `(mount path, response)` on the caller's mock
//! server, then runs `parse_from_url` against `entry_path` on that server.
//!
//! The mock → parser → parse skeleton lives here, once, so the WAF and
//! compression tests keep only their template construction and result
//! assertions. The server lifecycle and the entry-guard posture stay at the
//! call site (index bodies embed `mock.uri()`, and each test holds its own
//! `EnvGuard` disarmer across the call, exactly as before).
use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serve a single body at `mount_path`, then parse `entry_path`.
pub(crate) async fn serve_and_parse(
    mock: &MockServer,
    mount_path: &str,
    response: ResponseTemplate,
    entry_path: &str,
) -> Result<Vec<SitemapUrl>> {
    serve_many_and_parse(mock, vec![(mount_path, response)], entry_path).await
}

/// Serve several bodies, then parse `entry_path` (sitemap-index cases).
pub(crate) async fn serve_many_and_parse(
    mock: &MockServer,
    mounts: Vec<(&str, ResponseTemplate)>,
    entry_path: &str,
) -> Result<Vec<SitemapUrl>> {
    for (mount_path, response) in mounts {
        Mock::given(method("GET"))
            .and(path(mount_path))
            .respond_with(response)
            .mount(mock)
            .await;
    }
    let parser = SitemapParser::new().unwrap();
    parser
        .parse_from_url(
            &format!("{}{}", mock.uri(), entry_path),
            &CorrelationId::new(),
        )
        .await
}
