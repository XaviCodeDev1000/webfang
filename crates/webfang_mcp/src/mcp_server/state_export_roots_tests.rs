use super::*;
use serial_test::serial;
use tempfile::TempDir;
use tracing::field::{Field, Visit};
use tracing::Level;
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::prelude::*;
use webfang_core::domain::config::ScraperConfig;
use webfang_core::domain::CrawlerConfig;

// --- with_export_roots startup consistency check (#769) ---

static GLOBAL_SUBSCRIBER_INIT: std::sync::Once = std::sync::Once::new();

/// Set a global fmt subscriber (sink writer) so every `tracing` callsite
/// registers with `Interest::always()` instead of the `Interest::never()`
/// that gets cached process-wide when a thread hits a callsite with no
/// subscriber active — same guard as
/// `metrics.rs::ensure_global_subscriber`.
fn ensure_global_subscriber() {
    GLOBAL_SUBSCRIBER_INIT.call_once(|| {
        let _ = tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .finish(),
        );
    });
}

/// One captured tracing event: level, message, `(field, value)` pairs.
struct CapturedEvent {
    level: Level,
    message: String,
    fields: Vec<(String, String)>,
}

/// Collects `(field_name, value)` pairs as strings; the message field is
/// recorded as a plain field by `tracing`.
struct FieldCapture {
    sink: Arc<Mutex<Vec<(String, String)>>>,
    message: Arc<Mutex<Option<String>>>,
}

impl Visit for FieldCapture {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            // `format!("{:?}")` on the message args yields the quoted
            // string; strip the outer quotes so `.contains()` matches the
            // raw English message verbatim.
            let raw = format!("{value:?}");
            *self.message.lock().expect("message mutex") = Some(raw.trim_matches('"').to_string());
        } else {
            self.sink
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            *self.message.lock().expect("message mutex") = Some(value.to_string());
        } else {
            self.sink
                .lock()
                .expect("capture mutex")
                .push((field.name().to_string(), value.to_string()));
        }
    }
}

struct EventCaptureLayer {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

impl<S: tracing::Subscriber> Layer<S> for EventCaptureLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let fields = Arc::new(Mutex::new(Vec::new()));
        let message = Arc::new(Mutex::new(None));
        let mut visitor = FieldCapture {
            sink: Arc::clone(&fields),
            message: Arc::clone(&message),
        };
        event.record(&mut visitor);
        let message = message
            .lock()
            .expect("message mutex")
            .take()
            .unwrap_or_default();
        let fields = fields.lock().expect("capture mutex").split_off(0);
        let mut events = self.events.lock().expect("capture mutex");
        events.push(CapturedEvent {
            level: *event.metadata().level(),
            message,
            fields,
        });
    }
}

/// Run `body` under a capturing subscriber and return `(result, events)`.
fn capture_events_during<T>(body: impl FnOnce() -> T) -> (T, Vec<CapturedEvent>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(EventCaptureLayer {
        events: Arc::clone(&events),
    });
    let result = tracing::subscriber::with_default(subscriber, body);
    let captured = events.lock().expect("capture mutex").split_off(0);
    (result, captured)
}

/// Build a container whose `ScraperConfig::output_dir` is `output_dir`.
async fn container_with_output_dir(output_dir: PathBuf) -> Container {
    let crawler_config =
        CrawlerConfig::new(url::Url::parse("https://example.com").expect("valid url"));
    let scraper_config = ScraperConfig {
        output_dir,
        ..Default::default()
    };
    Container::new(crawler_config, scraper_config)
        .await
        .expect("create test container")
}

/// The `#769` warning is identified by its message; unrelated warns from
/// container construction (e.g. robots_fetcher init) never match it.
const OUTSIDE_ROOTS_WARN: &str = "outside the configured export roots";

#[tokio::test]
#[serial]
async fn with_export_roots_warns_when_output_dir_outside_roots() {
    ensure_global_subscriber();
    let root = TempDir::new().expect("create root temp dir");
    let outside = TempDir::new().expect("create outside temp dir");
    let outside_path = outside.path().display().to_string();
    let container = container_with_output_dir(outside.path().to_path_buf()).await;
    let state = McpState::new(container);

    let (_state, events) =
        capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));

    let warns: Vec<&CapturedEvent> = events
        .iter()
        .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
        .collect();
    assert_eq!(warns.len(), 1, "exactly one #769 warning must fire");
    let warn = warns[0];
    // Structured English fields, not string soup.
    let field = |name: &str| {
        warn.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        field("output_dir").as_deref(),
        Some(outside_path.as_str()),
        "output_dir field must carry the configured directory"
    );
    assert!(
        field("roots")
            .expect("roots field must be present")
            .contains(
                &*root
                    .path()
                    .file_name()
                    .expect("root has a name")
                    .to_string_lossy()
            ),
        "roots field must name the configured root"
    );
}

#[tokio::test]
#[serial]
async fn with_export_roots_no_warn_when_output_dir_inside_roots() {
    ensure_global_subscriber();
    let root = TempDir::new().expect("create temp dir");
    let container = container_with_output_dir(root.path().to_path_buf()).await;
    let state = McpState::new(container);

    let (_state, events) =
        capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));

    let stray = events
        .iter()
        .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
        .count();
    assert_eq!(stray, 0, "an output_dir under a root must not warn");
}

#[tokio::test]
#[serial]
async fn with_export_roots_no_warn_when_output_dir_relative() {
    ensure_global_subscriber();
    // Relative `output_dir` resolves against the server CWD — the same
    // policy as `validate_export_dir`: no warning, regardless of roots.
    // The container opens its crawl log lazily, so a relative path
    // performs no file I/O here (container.rs #606).
    let root = TempDir::new().expect("create root temp dir");
    let container = container_with_output_dir(PathBuf::from("output")).await;
    assert!(
        container.config().output_dir.is_relative(),
        "fixture must keep a relative output_dir"
    );
    let state = McpState::new(container);

    let (state, events) =
        capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));
    drop(state);

    let stray = events
        .iter()
        .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
        .count();
    assert_eq!(stray, 0, "a relative output_dir must not warn");
}

#[tokio::test]
#[serial]
async fn with_export_roots_warns_for_windows_rooted_output_dir_on_posix() {
    // G-8 (issue #1608): the old `is_absolute()` probe was platform-blind
    // — on POSIX a configured `C:\exports` is not absolute, so the #769
    // warn silently skipped it. `path_gate::classify` decides the shape
    // from the raw string, so the rooted form warns on every host. The
    // roots fixture is irrelevant to the verdict: a `C:\`-prefixed path
    // can never resolve under a Unix root either way.
    ensure_global_subscriber();
    let root = TempDir::new().expect("create root temp dir");
    let container = container_with_output_dir(PathBuf::from("C:\\exports")).await;
    let state = McpState::new(container);

    let (_state, events) =
        capture_events_during(|| state.with_export_roots(vec![root.path().to_path_buf()]));

    let warns = events
        .iter()
        .filter(|e| e.level == Level::WARN && e.message.contains(OUTSIDE_ROOTS_WARN))
        .count();
    assert_eq!(
        warns, 1,
        "a Windows-style rooted output_dir must warn on every host"
    );
}
