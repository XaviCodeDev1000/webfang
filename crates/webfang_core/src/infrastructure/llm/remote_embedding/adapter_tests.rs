use super::*;
use super::fixtures::*;
use crate::domain::auth_source::AuthSource;

// --- Construction contract ---

#[test]
fn missing_credential_fails_at_construction_not_first_call() {
    // Hermetic: clean the var — a leaked value from a sibling would flip
    // this to success (nextest isolates, plain cargo test does not).
    let _guard = webfang_test_utils::EnvGuard::clean(&["WEBFANG_TEST_REMOTE_MISSING_VAR"]);
    let cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_MISSING_VAR".to_string(),
    });
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
        Err(e) => e,
        Ok(_) => panic!("debe fallar en construcción"),
    };
    assert!(err.to_string().contains("test-remote"), "{err}");
    assert!(matches!(err, ProviderInitError::AuthFailed { .. }));
}

#[test]
fn missing_model_fails_at_construction() {
    // The key must resolve (constructor checks auth before model), so it
    // is set explicitly — never inherited from a sibling's guard.
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let mut cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
    });
    cfg.model = None;
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
        Err(e) => e,
        Ok(_) => panic!("model ausente debe fallar en construcción"),
    };
    assert!(err.to_string().contains("test-remote"), "{err}");
    assert!(matches!(err, ProviderInitError::InvalidBaseUrl { .. }));
}

#[test]
fn loopback_base_url_rejected_without_opt_in() {
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let mut cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
    });
    cfg.base_url =
        Url::parse(&format!("http://{LOOPBACK_HOST}:{LOOPBACK_PORT}/v1")).expect("url");
    cfg.allow_loopback = false;
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let err = match RemoteEmbeddingAdapter::with_http(cfg, client) {
        Err(e) => e,
        Ok(_) => panic!("loopback sin opt-in debe fallar en construcción"),
    };
    assert!(
        matches!(err, ProviderInitError::InvalidBaseUrl { .. }),
        "loopback sin opt-in debe ser InvalidBaseUrl, got: {err}"
    );
}

#[test]
fn loopback_base_url_constructs_with_opt_in() {
    let _guard = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let mut cfg = config_with(AuthSource::Env {
        var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
    });
    cfg.base_url =
        Url::parse(&format!("http://{LOOPBACK_HOST}:{LOOPBACK_PORT}/v1")).expect("url");
    cfg.allow_loopback = true;
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let adapter =
        RemoteEmbeddingAdapter::with_http(cfg, client).expect("loopback con opt-in construye");
    assert_eq!(adapter.id(), "test-remote");
}

#[test]
fn cold_adapter_reports_pin_or_zero_before_probe() {
    let env = webfang_test_utils::EnvGuard::with(&[("WEBFANG_TEST_REMOTE_KEY", "sk-test")]);
    let client = wreq::Client::builder()
        .build()
        .expect("plain test client builds");
    let pinned = RemoteEmbeddingAdapter::with_http(
        {
            let mut cfg = config_with(AuthSource::Env {
                var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
            });
            cfg.embedding_dim = Some(1536);
            cfg
        },
        client.clone(),
    )
    .expect("pinned adapter builds");
    assert_eq!(pinned.embedding_dim(), 1536);
    let unpinned = RemoteEmbeddingAdapter::with_http(
        config_with(AuthSource::Env {
            var: "WEBFANG_TEST_REMOTE_KEY".to_string(),
        }),
        client,
    )
    .expect("unpinned adapter builds");
    assert_eq!(unpinned.embedding_dim(), 0);
    drop(env);
}
