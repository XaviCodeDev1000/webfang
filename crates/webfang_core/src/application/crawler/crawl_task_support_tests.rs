use super::*;

use crate::application::crawler::ports::{
    ContentPipeline, CrawlResultCollector, FetchOutcome, LinkExtractorPort, PageFetcher,
    RobotsChecker,
};
use crate::application::rate_limiter::{RateLimiterConfig, SharedRateLimiter};
use crate::domain::cookie_bridge::CookieBridge;
use crate::domain::downloader_port::Cookie;
use crate::domain::session_port::{SessionId, SessionPort};
use crate::domain::CorrelationId;
use crate::infrastructure::crawler::UrlQueue;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use tokio::sync::RwLock as AsyncRwLock;
use tokio_util::sync::CancellationToken;
use url::Url;

/// #1610 (OBS-P2-4): the `crawl_page` outcome must survive the trip into
/// the JSONL — including `error_class`, recorded through
/// `tracing::field::display` because `Span::record` takes a `&dyn Value`
/// and has no `%` shorthand. That path is easy to get subtly wrong (Debug
/// quoting leaking into the field), so it is asserted on parsed JSON.
#[test]
fn page_outcome_reaches_the_trace_jsonl() {
    use crate::infrastructure::observability::FileTraceLayer;
    use tracing_subscriber::layer::SubscriberExt;

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("trace.jsonl");
    let layer = FileTraceLayer::new(path.clone()).expect("trace layer");
    let dispatch = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));

    tracing::dispatcher::with_default(&dispatch, || {
        let span = tracing::info_span!(
            "crawl_page",
            url = "https://example.com/page",
            outcome = tracing::field::Empty,
            http_status = tracing::field::Empty,
            error_class = tracing::field::Empty,
        );
        let _enter = span.enter();
        record_page_outcome(
            "error",
            None,
            Some(crate::error::ErrorClass::TransientBackoff),
        );
    });

    let closes: Vec<serde_json::Value> = std::fs::read_to_string(&path)
        .expect("trace file")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<serde_json::Value>(l).expect("jsonl line"))
        .filter(|v| v["record"] == "span_close")
        .collect();
    assert_eq!(closes.len(), 1, "exactly one span_close record");
    let fields = closes[0]["span_fields"]
        .as_object()
        .expect("span_fields must be an object");

    assert_eq!(
        fields["outcome"].as_str(),
        Some("error"),
        "outcome must reach the JSONL as a plain token"
    );
    assert_eq!(
        fields["error_class"].as_str(),
        Some("transient_backoff"),
        "ErrorClass must reach the JSONL as snake_case with no Debug              quoting (got {:?})",
        fields["error_class"]
    );
    assert!(
        !fields.contains_key("http_status"),
        "an outcome with no HTTP status must OMIT the key, not report an empty one"
    );
}

// ── Mock implementations ──

pub(super) enum FetchBehavior {
    Success(String, Vec<Cookie>),
    WafError(String),
    NetworkError(String),
}

pub(super) struct MockPageFetcher {
    pub(super) behavior: FetchBehavior,
    pub(super) final_url: Option<Url>,
}

impl PageFetcher for MockPageFetcher {
    fn fetch_page<'a>(
        &'a self,
        _url: &'a Url,
        _config: &'a crate::domain::CrawlerConfig,
    ) -> Pin<Box<dyn Future<Output = Result<FetchOutcome, CrawlError>> + Send + 'a>> {
        Box::pin(async move {
            match &self.behavior {
                FetchBehavior::Success(html, cookies) => Ok(FetchOutcome {
                    body: html.clone(),
                    final_url: self.final_url.clone().unwrap_or_else(|| _url.clone()),
                    cookies: cookies.clone(),
                    status: 200,
                }),
                FetchBehavior::WafError(msg) => Err(CrawlError::WafChallenge {
                    provider: msg.clone(),
                    kind: crate::domain::error::WafDetectionKind::BodySignature,
                    url: String::new(),
                }),
                FetchBehavior::NetworkError(msg) => Err(CrawlError::network(msg.clone(), None)),
            }
        })
    }
}

pub(super) struct MockRobotsChecker {
    pub(super) allowed: bool,
    pub(super) call_count: Arc<AtomicUsize>,
}

impl RobotsChecker for MockRobotsChecker {
    fn is_robots_allowed<'a>(
        &'a self,
        _url: &'a str,
        _domain: &'a str,
    ) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
        Box::pin(async move {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            self.allowed
        })
    }
}

pub(super) enum ExtractBehavior {
    Links(Vec<String>),
    Error(String),
}

pub(super) struct MockLinkExtractor {
    pub(super) behavior: ExtractBehavior,
}

impl LinkExtractorPort for MockLinkExtractor {
    fn extract_links(&self, _html: &str, _base_url: &str) -> Result<Vec<String>, CrawlError> {
        match &self.behavior {
            ExtractBehavior::Links(links) => Ok(links.clone()),
            ExtractBehavior::Error(msg) => Err(CrawlError::Parse(msg.clone())),
        }
    }
}

pub(super) enum PipelineBehavior {
    Filtered,
    Rejected(crate::domain::pipeline_item::RejectReason),
    Failed(crate::domain::error::ErrorClass),
}

pub(super) struct MockPipeline {
    pub(super) behavior: PipelineBehavior,
}

impl ContentPipeline for MockPipeline {
    fn execute_pipeline<'a>(
        &'a self,
        _item: ScrapedItem,
    ) -> Pin<Box<dyn Future<Output = StageOutcome> + Send + 'a>> {
        Box::pin(async move {
            match &self.behavior {
                PipelineBehavior::Filtered => StageOutcome::Filtered {
                    reason: crate::domain::pipeline_item::FilterReason::NonContentPath,
                },
                PipelineBehavior::Rejected(reason) => StageOutcome::Rejected {
                    reason: reason.clone(),
                },
                PipelineBehavior::Failed(class) => StageOutcome::Failed { class: *class },
            }
        })
    }
}

struct MockCollector {
    sent: Arc<Mutex<Vec<DiscoveredUrl>>>,
}

impl CrawlResultCollector for MockCollector {
    fn send_result<'a>(
        &'a self,
        url: DiscoveredUrl,
    ) -> Pin<Box<dyn Future<Output = Result<(), CrawlError>> + Send + 'a>> {
        Box::pin(async move {
            self.sent
                .lock()
                .map_err(|e| CrawlError::Internal(e.to_string()))?
                .push(url);
            Ok(())
        })
    }
}

pub(super) struct MockSessionPool {
    pub(super) acquire_result: Option<SessionId>,
    pub(super) success_calls: Arc<Mutex<Vec<(String, SessionId)>>>,
}

impl SessionPort for MockSessionPool {
    fn acquire(&self, _domain: &str) -> Option<SessionId> {
        self.acquire_result
    }
    fn report_success(&self, domain: &str, session: SessionId) {
        if let Ok(mut calls) = self.success_calls.lock() {
            calls.push((domain.to_string(), session));
        }
    }
    fn report_failure(&self, _domain: &str, _session: SessionId, _status: u16) {}
}

// ── Test helpers ──

pub(super) struct TestCtxBuilder {
    fetcher: Arc<dyn PageFetcher>,
    robots: Arc<dyn RobotsChecker>,
    link_extractor: Arc<dyn LinkExtractorPort>,
    pipeline: Option<Arc<dyn ContentPipeline>>,
    collector: Arc<dyn CrawlResultCollector>,
    session_pool: Option<Arc<dyn SessionPort>>,
    rate_limiter: Option<SharedRateLimiter>,
    cancel_token: CancellationToken,
    ignore_robots: bool,
    max_depth: u8,
}

impl TestCtxBuilder {
    pub(super) fn new(collector: Arc<dyn CrawlResultCollector>) -> Self {
        Self {
            fetcher: Arc::new(MockPageFetcher {
                behavior: FetchBehavior::Success(
                    "<html><body>hello</body></html>".to_string(),
                    Vec::new(),
                ),
                final_url: None,
            }),
            robots: Arc::new(MockRobotsChecker {
                allowed: true,
                call_count: Arc::new(AtomicUsize::new(0)),
            }),
            link_extractor: Arc::new(MockLinkExtractor {
                behavior: ExtractBehavior::Links(Vec::new()),
            }),
            pipeline: None,
            collector,
            session_pool: None,
            rate_limiter: None,
            cancel_token: CancellationToken::new(),
            ignore_robots: false,
            max_depth: 3,
        }
    }

    pub(super) fn fetcher(mut self, f: Arc<dyn PageFetcher>) -> Self {
        self.fetcher = f;
        self
    }
    pub(super) fn robots(mut self, r: Arc<dyn RobotsChecker>) -> Self {
        self.robots = r;
        self
    }
    pub(super) fn link_extractor(mut self, le: Arc<dyn LinkExtractorPort>) -> Self {
        self.link_extractor = le;
        self
    }
    pub(super) fn pipeline(mut self, p: Arc<dyn ContentPipeline>) -> Self {
        self.pipeline = Some(p);
        self
    }
    pub(super) fn session_pool(mut self, sp: Arc<dyn SessionPort>) -> Self {
        self.session_pool = Some(sp);
        self
    }
    pub(super) fn rate_limiter(mut self, rl: SharedRateLimiter) -> Self {
        self.rate_limiter = Some(rl);
        self
    }
    pub(super) fn cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }
    pub(super) fn ignore_robots(mut self, v: bool) -> Self {
        self.ignore_robots = v;
        self
    }
    pub(super) fn max_depth(mut self, d: u8) -> Self {
        self.max_depth = d;
        self
    }

    pub(super) fn build(self) -> Arc<CrawlTaskCtx> {
        let seed_url = Url::parse("https://example.com").expect("valid seed URL");
        let mut config = crate::domain::CrawlerConfig::new(seed_url);
        config.max_depth = self.max_depth;

        let rate_limiter = self.rate_limiter.unwrap_or_else(|| {
            SharedRateLimiter::new(&RateLimiterConfig::new(1, 100))
                .expect("valid rate limiter config")
        });

        Arc::new(CrawlTaskCtx {
            config: Arc::new(config),
            correlation_id: CorrelationId::new(),
            queue: Arc::new(UrlQueue::new()),
            rate_limiter,
            cancel_token: self.cancel_token,
            session_pool: self.session_pool,
            ignore_robots: self.ignore_robots,
            robots_checker: self.robots,
            error_count: Arc::new(AtomicUsize::new(0)),
            error_breakdown: Arc::new([
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
                AtomicUsize::new(0),
            ]),
            pages_crawled: Arc::new(AtomicU64::new(0)),
            collector: self.collector,
            cookie_bridge: Arc::new(AsyncRwLock::new(CookieBridge::new())),
            banned_domains: Arc::new(RwLock::new(Vec::new())),
            fetcher: self.fetcher,
            link_extractor: self.link_extractor,
            pipeline: self.pipeline,
            content_sink: None,
        })
    }
}

pub(super) fn test_url(url: &str, depth: u8) -> DiscoveredUrl {
    DiscoveredUrl::html(
        Url::parse(url).expect("valid test URL"),
        depth,
        Url::parse("https://example.com").expect("valid parent URL"),
    )
}

pub(super) fn mock_collector() -> (
    Arc<dyn CrawlResultCollector>,
    Arc<Mutex<Vec<DiscoveredUrl>>>,
) {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let collector = Arc::new(MockCollector {
        sent: Arc::clone(&sent),
    });
    (collector, sent)
}
