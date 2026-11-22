use super::arrange::*;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use wiremock::MockServer;

/// Mock SessionPort for testing.
struct MockSessionPort {
    /// Controls what acquire returns: Some(SessionId(0)) or None (banned).
    banned: std::sync::atomic::AtomicBool,
    /// Counts calls to report_success and report_failure.
    success_count: AtomicUsize,
    failure_count: AtomicUsize,
    last_failure_status: std::sync::atomic::AtomicU16,
}

impl MockSessionPort {
    fn new(banned: bool) -> Arc<Self> {
        Arc::new(Self {
            banned: std::sync::atomic::AtomicBool::new(banned),
            success_count: AtomicUsize::new(0),
            failure_count: AtomicUsize::new(0),
            last_failure_status: std::sync::atomic::AtomicU16::new(0),
        })
    }
}

impl SessionPort for MockSessionPort {
    fn acquire(&self, _domain: &str) -> Option<SessionId> {
        if self.banned.load(Ordering::SeqCst) {
            None
        } else {
            Some(SessionId(0))
        }
    }

    fn report_success(&self, _domain: &str, _session: SessionId) {
        self.success_count.fetch_add(1, Ordering::SeqCst);
    }

    fn report_failure(&self, _domain: &str, _session: SessionId, status: u16) {
        self.failure_count.fetch_add(1, Ordering::SeqCst);
        self.last_failure_status.store(status, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn test_acquire_banned_returns_domain_banned_error() {
    let pool = MockSessionPort::new(true);
    let client = default_client().with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    let result = client.get("https://example.com/page").await;

    assert!(result.is_err());
    match result.unwrap_err() {
        HttpError::DomainBanned(domain) => assert_eq!(domain, "example.com"),
        other => panic!("expected DomainBanned, got {other:?}"),
    }
}

#[tokio::test]
async fn test_acquire_healthy_proceeds_with_request() {
    let pool = MockSessionPort::new(false);
    let client = default_client().with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    // Use a non-existent URL — we just want to verify it doesn't return DomainBanned
    let result = client.get("https://httpbin.org/get").await;
    // Should NOT be DomainBanned — could be any other error (network, etc.)
    if let Err(HttpError::DomainBanned(_)) = &result {
        panic!("should not be DomainBanned for healthy pool")
    }
}

#[tokio::test]
async fn test_report_outcome_success_calls_report_success() {
    let pool = MockSessionPort::new(false);
    let client = default_client().with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    let mock_server = MockServer::start().await;
    mount_body(&mock_server, "/", 200, "ok").await;

    let _ = get_route(&client, &mock_server, "/").await;

    assert_eq!(pool.success_count.load(Ordering::SeqCst), 1);
    assert_eq!(pool.failure_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_report_outcome_403_calls_report_failure() {
    let pool = MockSessionPort::new(false);
    let client = retry_client(1).with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/", 403).await;

    let _ = get_route(&client, &mock_server, "/").await;

    assert_eq!(pool.failure_count.load(Ordering::SeqCst), 1);
    assert_eq!(pool.last_failure_status.load(Ordering::SeqCst), 403);
}

#[tokio::test]
async fn test_report_outcome_404_no_report_failure() {
    let pool = MockSessionPort::new(false);
    let client = default_client().with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    let mock_server = MockServer::start().await;
    mount_status(&mock_server, "/notfound", 404).await;

    let _ = get_route(&client, &mock_server, "/notfound").await;

    assert_eq!(pool.success_count.load(Ordering::SeqCst), 0);
    assert_eq!(pool.failure_count.load(Ordering::SeqCst), 0);
}

#[test]
fn test_builder_sets_session_pool() {
    let pool = MockSessionPort::new(false);
    let client = default_client().with_session_pool(Arc::clone(&pool) as Arc<dyn SessionPort>);

    assert!(client.session_pool.is_some());
}

#[test]
fn test_builder_without_session_pool() {
    let client = default_client();

    assert!(client.session_pool.is_none());
}
