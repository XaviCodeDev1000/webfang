//! Shared arrange fixtures for the `connect` tests — extracted verbatim from
//! the in-file `tests` mod (ADR-0021 Slice 1 headroom under the 500-line file
//! ceiling; pure move, no assertion changed). The parent's `tests` module
//! glob-imports this one.

use std::path::Path;

use super::{display_for_id, ProviderConfig};
// `write_provider` is only consumed by the `#[cfg(unix)]` file-mode fixtures
// below; importing it on Windows fails the cross-lint under -D unused-imports.
#[cfg(unix)]
use super::write_provider;
use wiremock::matchers::{method, path as path_matcher};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A wiremock server answering `POST /embeddings` with a 3-wide vector.
pub(super) async fn serve_embeddings() -> MockServer {
    let server = MockServer::start().await;
    let body = r#"{"object":"list","data":[{"object":"embedding","index":0,"embedding":[0.0,0.1,0.2]}],"model":"nomic-embed-text","usage":{"prompt_tokens":0,"total_tokens":0}}"#;
    Mock::given(method("POST"))
        .and(path_matcher("/embeddings"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(body)
                .append_header("content-type", "application/json"),
        )
        .mount(&server)
        .await;
    server
}

/// Canonical embedding provider for arrange blocks: the anonymous,
/// loopback-forbidden, dim-unpinned shape `connect` builds. Tests that
/// need another id, endpoint, model or loopback flag mutate the returned
/// value — the arrange stays one line either way.
pub(super) fn test_provider(id: &str) -> ProviderConfig {
    ProviderConfig {
        id: id.into(),
        display_name: display_for_id(id),
        base_url: url::Url::parse("https://api.example.test/v1").expect("url"),
        ..crate::domain::providers::test_provider_config()
    }
}

/// Fresh `config.toml` path inside a temp dir. The dir is returned so it
/// outlives the test body.
pub(super) fn temp_config() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    (dir, path)
}

/// Seed `path` with `contents` for merge/gate tests.
pub(super) fn seed_config(path: &Path, contents: &str) {
    std::fs::write(path, contents).expect("seed config");
}

/// Provider pointed at the mock embeddings server, with the loopback
/// opt-in the guarded production client requires for it.
pub(super) fn loopback_provider(server: &MockServer, id: &str) -> ProviderConfig {
    let mut provider = test_provider(id);
    provider.base_url = server.uri().parse().expect("mock uri");
    provider.allow_loopback = true;
    provider
}

/// First write of the canonical provider for the file-mode tests: every
/// mode test starts from the same written record, then chmods it and
/// rewrites.
#[cfg(unix)]
pub(super) fn first_write_canonical() -> (tempfile::TempDir, std::path::PathBuf, ProviderConfig) {
    let (dir, path) = temp_config();
    let provider = test_provider("connect-a1b2");
    write_provider(&path, &provider).expect("first write");
    (dir, path, provider)
}

/// Second write plus the mode assertion shared by the file-mode tests:
/// the atomic rewrite must keep the operator's mode, wide or narrow.
#[cfg(unix)]
pub(super) fn assert_second_write_keeps_mode(
    path: &Path,
    provider: &ProviderConfig,
    mode: u32,
    context: &str,
) {
    use std::os::unix::fs::PermissionsExt;
    write_provider(path, provider).expect("second write");
    let actual = std::fs::metadata(path).expect("stat").permissions().mode();
    assert_eq!(actual & 0o777, mode, "{context}");
}

/// Names of the entries directly inside `dir` that satisfy `keep`, for
/// the temp-sweep assertions (a failed write leaves none; a success
/// sweeps stale siblings).
pub(super) fn dir_file_names_matching(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("list the parent directory")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| keep(name))
        .collect()
}

/// Config path plus a pre-planted temp candidate for the symlink-safety
/// tests.
pub(super) fn planted_temp() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let (dir, target) = temp_config();
    let planted = dir.path().join(".config.toml.connect-planted.tmp");
    std::fs::write(&planted, b"PLANT\n").expect("seed the plant");
    (dir, target, planted)
}
