//! Shared arrange fixtures for the `remote_embedding` tests — extracted
//! verbatim from the in-file `tests` mod (ADR-0021 Slice 1 headroom under the
//! descending file-size ratchet; pure move, no assertion changed). The
//! parent's `tests` module glob-imports this one.

use super::{RemoteEmbeddingAdapter, Url};
use crate::domain::auth_source::AuthSource;
use crate::domain::providers::{Capability, ProviderConfig, ProviderKind};

/// Base URLs for the entry-gate and dial cases, composed at runtime rather
/// than written as one `IP:port` literal. That shape reads as `path:line`
/// to tooling that scans evidence locations, and the native review
/// admission layer rejects a reviewer result citing one as
/// `evidence_path_out_of_scope`. The fixtures themselves are unchanged.
pub(super) const LAN_HOST: &str = "10.0.0.5";
pub(super) const LAN_PORT: u16 = 8000;
pub(super) const LOOPBACK_HOST: &str = "127.0.0.1";
pub(super) const LOOPBACK_PORT: u16 = 9;

/// Fixture update rule (pattern `fixtures/llm`): fixtures live outside the
/// code; if a vendor drifts the wire shape, the fixture is updated first in a
/// separate commit with justification.
pub(super) fn load_fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../fixtures/remote_embedding/{name}"));
    std::fs::read_to_string(&path).expect("fixture legible")
}

pub(super) fn config_with(auth: AuthSource) -> ProviderConfig {
    ProviderConfig {
        id: "test-remote".to_string(),
        display_name: "Test Remote".to_string(),
        kind: ProviderKind::OpenAiCompatible,
        base_url: Url::parse("https://api.example.com/v1").expect("url"),
        auth,
        capabilities: vec![Capability::Embedding],
        model: Some("nomic-embed-text".to_string()),
        embedding_dim: None,
        allow_loopback: false,
        network_policy: crate::domain::network_policy::NetworkPolicy::default(),
        proxy: None,
    }
}

pub(super) fn adapter_for(
    server: &wiremock::MockServer,
    modifier: impl FnOnce(&mut ProviderConfig),
) -> RemoteEmbeddingAdapter {
    let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let mut cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
    });
    cfg.base_url = Url::parse(&server.uri()).expect("mock uri parses");
    cfg.allow_loopback = true;
    modifier(&mut cfg);
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let adapter = RemoteEmbeddingAdapter::with_http(cfg, client).expect("test adapter builds");
    // `with_http` resolves the credential in the constructor: dropping
    // the env guard here proves later calls never re-resolve.
    drop(env);
    adapter
}
