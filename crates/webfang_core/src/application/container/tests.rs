use super::*;
use crate::domain::config::ScraperConfig;
use crate::domain::CrawlerConfig;
use tempfile::TempDir;

/// Create a Container with default configs backed by a TempDir.
/// Returns `(TempDir, Container)` — caller keeps `tmp` alive for the test scope.
async fn make_test_container() -> (TempDir, Container) {
    let tmp = TempDir::new().unwrap();
    let crawler_config = CrawlerConfig::new(url::Url::parse("https://example.com").unwrap());
    let scraper_config = ScraperConfig {
        output_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let container = Container::new(crawler_config, scraper_config)
        .await
        .unwrap();
    (tmp, container)
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_wires_crawl_result_repository() {
    let (_tmp, container) = make_test_container().await;
    let repo = container.crawl_result_repository();
    assert!(
        repo.is_some(),
        "crawl_result_repository() debe retornar Some"
    );
}

/// #1106 — sink construction moved to the blocking pool inside
/// `with_elastic_ingestion` (`create_pool` stats/creates directories,
/// `StreamRepository::new` does create_dir_all + File::create). The
/// wiring contract must hold after the move: sink active, file created.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_with_elastic_ingestion_wires_stream_sink() {
    let (tmp, container) = make_test_container().await;
    let vectors_path = tmp.path().join("vectors.jsonl");

    let mut opts = CrawlOptions::default();
    opts.elastic.output_vectors = Some(vectors_path.to_string_lossy().to_string());

    let container = container
        .with_elastic_ingestion(&opts)
        .await
        .expect("stream sink wiring must succeed");
    assert!(
        container.elastic_ingestion.is_some(),
        "--output-vectors must activate elastic ingestion"
    );
    assert!(
        vectors_path.exists(),
        "StreamRepository::new must create the JSONL file"
    );
}

/// F-52-b (#1277): the scrape path must carry `--js-wait` into the
/// downloader spec — same propagation guarantee as the engine path,
/// or scrapes silently settle differently from crawls.
#[test]
fn scrape_downloader_spec_propagates_post_load_wait() {
    use crate::application::http_client::HttpClientConfig;
    use crate::domain::post_load_wait::PostLoadWait;

    for mode in [
        PostLoadWait::Idle,
        PostLoadWait::Fixed(400),
        PostLoadWait::None,
    ] {
        let mut opts = CrawlOptions::default();
        opts.network.post_load_wait = mode;
        let spec = Container::scrape_downloader_spec(&opts, &HttpClientConfig::default(), None);
        assert_eq!(
            spec.post_load_wait, mode,
            "--js-wait {mode} must reach the scrape downloader spec"
        );
    }
}

// --- SSRF guard wiring (ADR-0012 sub-slice 3.C) ---

/// Wiring: `Container::new` arms the process-wide SSRF guard registry.
/// Strict-TDD RED without the step-6b arm (fresh nextest process starts
/// with the registry unarmed; the fallback does not count as armed).
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_arms_ssrf_guard() {
    let (_tmp, _container) = make_test_container().await;
    assert!(
        crate::domain::ssrf_guard::ssrf_guard_armed(),
        "Container::new debe armar el registry del SsrfGuard"
    );
    // Behavioral proof the armed guard is the full guard: it builds a
    // client end-to-end (redirect policy + validating resolver are
    // applied inside `DefaultSsrfGuard::secure_client`). The concrete
    // type is guaranteed at the arming site by the compiler.
    let builder = wreq::Client::builder();
    let _client = crate::domain::ssrf_guard::ssrf_guard()
        .secure_client(builder)
        .build()
        .expect("armed guard builds a client");
}

/// Keep-first idempotency (#996): `Container::new` must not replace a
/// guard the registry already holds. The winner is captured after this
/// test's own arm rather than assumed, because the registry is
/// process-global and unresettable: nextest starts each test unarmed (so
/// the winner is exactly our arm and the test also proves it
/// survives `Container::new`), while plain `cargo test` shares one
/// process where a sibling may have armed it first.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_ssrf_guard_arming_is_keep_first() {
    // Arm the production default guard, not a no-op sentinel: the
    // registry is process-global and keep-first, so a behavior-less
    // sentinel armed here would win the race and leave every later
    // client in this `cargo test` process unguarded (#2626).
    let guard: std::sync::Arc<dyn crate::domain::ssrf_guard::SsrfGuard> =
        std::sync::Arc::new(crate::domain::ssrf_guard::DefaultSsrfGuard);
    crate::domain::ssrf_guard::set_ssrf_guard(guard);
    // Captured after our own arm, so this is a registry value and not
    // the fallback: the armed guard when this test armed an unarmed
    // registry, or a sibling's guard in a shared `cargo test` process.
    let winner = crate::domain::ssrf_guard::ssrf_guard();

    let (_tmp, _container) = make_test_container().await;

    assert!(
        std::sync::Arc::ptr_eq(&crate::domain::ssrf_guard::ssrf_guard(), &winner),
        "Container::new no debe reemplazar un guard ya armado (keep-first, #996)"
    );
}

// --- Session-pool composition-root seam (ADR-0012-B sub-slice 3.F, #1075) ---

/// `build_crawl_session_pool` is the sanctioned place where the domain
/// `SessionPoolConfig` DTO becomes an `Arc<dyn SessionPort>`. The returned
/// port must serve a healthy acquire immediately (pure construction — no
/// wreq/FFI, so this test runs under Miri too).
#[test]
fn build_crawl_session_pool_returns_usable_port() {
    let pool = build_crawl_session_pool(SessionPoolConfig::default());
    assert!(
        pool.acquire("example.com").is_some(),
        "the port built by the composition-root seam must serve a healthy acquire"
    );
}

// --- State-store composition-root seam (ADR-0012-B 3.H, #1097) ---

/// `build_state_store` derives `<state_dir>/<domain>.json` without I/O.
///
/// Moved with the factory from `cli::scrape_flow::create_state_store`
/// (#1097): the path contract is unchanged, only the return is erased to
/// `Arc<dyn StateStorePort>`.
#[test]
fn test_build_state_store_derives_domain_path() {
    let temp_dir = TempDir::new().unwrap();
    let domain = "example.com";
    let store = build_state_store(temp_dir.path().to_path_buf(), domain);
    let state_file = temp_dir.path().join("example.com.json");
    assert_eq!(store.get_state_path(), state_file);
}

/// Pins the factory's lazy/infallible contract (#1097): construction
/// performs no I/O, so it succeeds even when `state_dir` is a regular
/// file where no directory could ever be created.
///
/// Moved with the factory from
/// `test_create_state_store_returns_ok_even_when_state_dir_is_a_file` —
/// not weakened: the state directory is still created only later, on
/// `save`. If the factory ever becomes eager (e.g. `create_dir_all` up
/// front), this test must fail and the callers regain an error route.
#[test]
fn test_build_state_store_succeeds_even_when_state_dir_is_a_file() {
    // Arrange: a regular file where a state directory would need to be —
    // an impossible location for any real directory creation.
    let temp_dir = TempDir::new().expect("tempdir should be created");
    let blocker = temp_dir.path().join("not_a_dir");
    std::fs::write(&blocker, "i am a file, not a directory")
        .expect("blocker file should be written");

    // Act: infallible by contract — no Result to unwrap, a panic here
    // would be the failure.
    let store = build_state_store(blocker.clone(), "example.com");

    // Assert: the path still derives under the (unusable) dir; no I/O
    // was attempted at creation time.
    assert_eq!(
        store.get_state_path(),
        blocker.join("example.com.json"),
        "build_state_store must be infallible (lazy): it performs no I/O at creation time"
    );
}

// --- Sitemap-parser composition-root seam (ADR-0012-B, #1082 follow-up) ---

/// `build_sitemap_parser` is the sanctioned place where the domain
/// `SitemapConfig` DTO + TLS profile become an `Arc<dyn SitemapParserPort>`.
/// Unlike the session-pool seam, construction builds the wreq fetch client
/// (btls-sys FFI), so this test follows the established Miri-ignore
/// pattern of every other client-building container test. The assertion is
/// that the seam succeeds and the erased port type is constructible; no
/// network call is made.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]
fn build_sitemap_parser_returns_usable_port() {
    let parser = build_sitemap_parser(
        crate::domain::crawler_port::SitemapConfig::default(),
        wreq_util::Profile::Chrome145,
    );
    assert!(
        parser.is_ok(),
        "the composition-root seam must build an erased sitemap port"
    );
    // The explicit binding proves the erased `Arc<dyn SitemapParserPort>`
    // return is usable (Send + Sync come with the trait's supertraits).
    let _port: Arc<dyn crate::domain::crawler_port::sitemap::SitemapParserPort> = parser.unwrap();
}

// --- Tests for expanded Container (Phase 3: DI with port/adapter) ---

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_provides_all_required_services() {
    let (_tmp, container) = make_test_container().await;

    // Verify all core services are available (non-optional accessors)
    let _ = container.http_client();
    let _ = container.deduplicator();
    let _ = container.credential_store();
    // Optional services
    assert!(
        container.rate_limiter().is_some(),
        "rate_limiter must be available"
    );
    assert!(
        container.crawl_result_repository().is_some(),
        "crawl_result_repository must be available"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_http_client_implements_port() {
    let (_tmp, container) = make_test_container().await;

    // Verify http_client is usable as a port trait object
    let client = container.http_client();
    let _port: &dyn crate::domain::ports::HttpClientPort = client.as_ref();
    // If this compiles, the port trait is properly implemented
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_config_accessors() {
    let (tmp, container) = make_test_container().await;

    // Verify config accessors work
    assert_eq!(
        container.config().output_dir,
        tmp.path(),
        "config should expose output_dir"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn test_container_clone_shares_services() {
    let (_tmp, container) = make_test_container().await;
    let container2 = container.clone();

    // Both clones share the same Arc'd services
    assert!(Arc::ptr_eq(
        container.http_client(),
        container2.http_client()
    ));
}

// --- Tests for the optional semantic-cleaner port (REQ-05) ---

/// In-crate fake cleaner — the defining crate may implement its own sealed
/// trait (`private::Sealed`), which external crates cannot. Enables a CI
/// test of the Container cleaner port without loading an ONNX model.
struct FakeCleaner;

impl crate::domain::semantic_cleaner::private::Sealed for FakeCleaner {}

impl crate::domain::semantic_cleaner::SemanticCleaner for FakeCleaner {
    fn clean<'a>(
        &'a self,
        _url: &'a str,
        _html: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<Vec<crate::domain::DocumentChunk>, crate::error::SemanticError>,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn max_chars(&self) -> usize {
        512
    }

    fn is_ready(&self) -> bool {
        true
    }
}

/// REQ-05 (absence): a freshly built container reports no cleaner.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn cleaner_absent_by_default() {
    let (_tmp, container) = make_test_container().await;
    assert!(
        container.cleaner().is_none(),
        "cleaner() must be None when no cleaner was injected"
    );
}

/// REQ-05 (injection): `with_cleaner` makes the accessor report present.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn with_cleaner_sets_cleaner() {
    let (_tmp, container) = make_test_container().await;
    let container = container.with_cleaner(Arc::new(FakeCleaner));
    assert!(
        container.cleaner().is_some(),
        "cleaner() must be Some after with_cleaner injection"
    );
}

/// Stub LLM port — answers any request with an empty records object.
struct StubLlm;

impl crate::domain::llm_port::LlmPort for StubLlm {
    fn send_completion<'a>(
        &'a self,
        _request: crate::domain::llm_port::LlmRequest,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = std::result::Result<
                        crate::domain::llm_port::LlmResponse,
                        crate::error::ScraperError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async {
            Ok(crate::domain::llm_port::LlmResponse {
                content: "{}".into(),
                input_tokens: 1,
                output_tokens: 1,
            })
        })
    }
}

/// #789 (absence): a freshly built container reports no LLM port.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn llm_port_absent_by_default() {
    let (_tmp, container) = make_test_container().await;
    assert!(
        container.llm_port().is_none(),
        "llm_port() must be None when no port was injected"
    );
}

/// #789 (injection): `with_llm_port` makes the accessor report present.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn with_llm_port_sets_llm_port() {
    let (_tmp, container) = make_test_container().await;
    let container = container.with_llm_port(Arc::new(StubLlm));
    assert!(
        container.llm_port().is_some(),
        "llm_port() must be Some after with_llm_port injection"
    );
}

// --- Vault note reader port (ADR-0012-B sub-slice 3.I, #1071) ---

/// In-crate stub reader — proves the seam accepts any
/// `Arc<dyn VaultNoteReader>` without touching the filesystem.
#[derive(Debug, Default)]
struct StubVaultNoteReader;

impl crate::domain::note_repository::VaultNoteReader for StubVaultNoteReader {
    fn read_vault_notes(
        &self,
        _vault_path: &std::path::Path,
    ) -> Result<Vec<crate::domain::note_repository::VaultNote>, crate::error::ScraperError> {
        Ok(Vec::new())
    }
}

/// Absence: a freshly built container reports no vault note reader.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn vault_note_reader_absent_by_default() {
    let (_tmp, container) = make_test_container().await;
    assert!(
        container.vault_note_reader().is_none(),
        "vault_note_reader() must be None when no reader was injected"
    );
}

/// Injection: `with_vault_note_reader` makes the accessor report present.
#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]
async fn with_vault_note_reader_sets_reader() {
    let (_tmp, container) = make_test_container().await;
    let container = container.with_vault_note_reader(Arc::new(StubVaultNoteReader));
    assert!(
        container.vault_note_reader().is_some(),
        "vault_note_reader() must be Some after with_vault_note_reader injection"
    );
}

/// Regression for #544: the elastic-ingestion semaphore MUST be sized in
/// BYTES (the unit `ResourceDownloader` consumes via `acquire_many`), not in
/// a count of resources. With explicit overrides we fix the RAM budget and
/// assert the produced permit count covers `max_resource_bytes`.
///
/// On the buggy wiring `permits = ram_budget/max_resource` (= 81 for these
/// values) which is smaller than `max_resource_bytes` (26_214_400) — the
/// assertion fails. After the fix `permits == ram_budget_bytes` (in bytes),
/// which is larger, so it passes.
#[test]
fn test_elastic_semaphore_sized_in_bytes() {
    let overrides = ElasticOverrides {
        cpu_cores: None,
        ram_budget_bytes: Some(2 * 1024 * 1024 * 1024), // 2 GiB
        max_resource_bytes: Some(25 * 1024 * 1024),     // 25 MiB
        db_path: None,
    };
    let config = ElasticConfig::resolve(&overrides);
    let permits = Container::build_ingestion_semaphore_permits(config.ram_budget_bytes);

    assert!(
        permits >= config.max_resource_bytes as usize,
        "el semáforo debe inicializarse en BYTES (permits={permits}) >= \
         max_resource_bytes={}",
        config.max_resource_bytes
    );
    // Sanity: with 2 GiB budget the permit count equals the byte budget.
    assert_eq!(permits, 2 * 1024 * 1024 * 1024);
}
