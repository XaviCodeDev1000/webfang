    /// #1615 (F11 / G-3 / G-4 / G-5): these tests drive real fetches against
    /// wiremock's 127.0.0.1, which the literal-IP entry guard now refuses.
    /// `EnvGuard::entry_guard_off()` disarms the guard for its lifetime — the
    /// named constructor is the repo's rule, so each call site stays one line.
    fn entry_off() -> webfang_test_utils::EnvGuard {
        webfang_test_utils::EnvGuard::entry_guard_off()
    }
    use super::*;
    use crate::domain::CrawlError;
    // parse_sitemap stays an infrastructure fn (quick_xml machinery);
    // the application re-export was dismantled (ADR-0012-B unit 2).
    use crate::infrastructure::crawler::parse_sitemap;
    #[cfg(not(miri))]
    use std::time::Duration;

    #[test]
    fn test_parse_sitemap_xml() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
    <url>
        <loc>https://example.com/page2</loc>
    </url>
    <url>
        <loc>https://example.com/page3</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0].url.as_str(), "https://example.com/page1");
        assert_eq!(urls[1].url.as_str(), "https://example.com/page2");
        assert_eq!(urls[2].url.as_str(), "https://example.com/page3");
    }

    #[test]
    fn test_parse_sitemap_with_cdata() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc><![CDATA[https://example.com/page1]]></loc></url>
    <url><loc>https://example.com/page2</loc></url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .any(|u| u.url.as_str() == "https://example.com/page1"));
        assert!(urls
            .iter()
            .any(|u| u.url.as_str() == "https://example.com/page2"));
    }

    #[test]
    fn test_parse_sitemap_with_namespaces() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
        xmlns:xhtml="http://www.w3.org/1999/xhtml">
    <url>
        <loc>https://example.com/page1</loc>
    </url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].url.as_str(), "https://example.com/page1");
    }

    #[test]
    fn test_parse_sitemap_xml_empty() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert!(urls.is_empty());
    }

    #[test]
    fn test_parse_sitemap_invalid_xml() {
        // stabilization-sitemap-regression: non-XML content is a parse failure,
        // NOT graceful degradation to empty. Empty ≡ valid structure with zero
        // URLs; garbage input must surface as an error so the CLI maps it to
        // exit 69 instead of silently reporting "no URLs found" (exit 2).
        let xml = "not xml at all";
        let base = Url::parse("https://example.com").unwrap();
        let result = parse_sitemap(xml, &base);
        assert!(
            matches!(result, Err(CrawlError::Parse(_))),
            "expected Parse error for non-XML input, got {result:?}"
        );
    }

    #[test]
    fn test_parse_sitemap_relative_urls_resolved() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    <url><loc>/page1</loc></url>
    <url><loc>https://external.com/page2</loc></url>
</urlset>"#;

        let base = Url::parse("https://example.com").unwrap();
        let urls = parse_sitemap(xml, &base).unwrap();
        assert_eq!(urls.len(), 2);
        assert!(urls
            .iter()
            .any(|u| u.url.as_str() == "https://example.com/page1"));
        assert!(urls
            .iter()
            .any(|u| u.url.as_str() == "https://external.com/page2"));
    }

    // #289 acceptance tests: the DOM discovery path must honor CrawlerConfig
    // timeouts. Both build a real wreq client (btls-sys FFI), hence not(miri).

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_single_fetch_respects_request_timeout() {
        let _entry_off = entry_off();
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // wiremock completes TCP instantly then delays the HTTP response, so this
        // exercises the request timeout (timeout_secs), not the connect timeout.
        let server = MockServer::start().await;
        Mock::given(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_string("<html></html>")
                    .set_delay(Duration::from_secs(10)),
            )
            .mount(&server)
            .await;

        let seed = Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed).timeout_secs(2).build();

        let start = std::time::Instant::now();
        let result =
            discover_urls_single_fetch(&server.uri(), &config, &CorrelationId::new()).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("slow response should time out");
        assert!(
            elapsed < Duration::from_secs(6),
            "2s request timeout should fire well before 6s, took {elapsed:?}"
        );
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("timed out") || msg.contains("timeout"),
            "expected timeout message, got: {msg}"
        );
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_single_fetch_respects_connect_timeout() {
        let _entry_off = entry_off();
        use tokio::net::TcpListener;

        // TLS blackhole: accept TCP connections and hold them open without ever
        // completing the TLS handshake. wiremock cannot simulate this (it always
        // finishes TCP+TLS), so a raw listener is used. This exercises the connect
        // timeout, which covers TCP+TLS establishment (reqwest semantics).
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                // Keep the socket alive and silent so the handshake never finishes.
                tokio::spawn(async move {
                    let _socket = socket;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });

        let target = format!("https://127.0.0.1:{port}");
        let seed = Url::parse(&target).unwrap();
        let config = CrawlerConfig::builder(seed).timeout_secs(2).build();

        let start = std::time::Instant::now();
        let result = discover_urls_single_fetch(&target, &config, &CorrelationId::new()).await;
        let elapsed = start.elapsed();

        let err = result.expect_err("TLS blackhole should fail to connect");
        assert!(
            elapsed < Duration::from_secs(6),
            "2s connect timeout should fire well before 6s, took {elapsed:?}"
        );
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("client error (connect)")
                || msg.contains("connection")
                || msg.contains("connect")
                || msg.contains("timed out")
                || msg.contains("timeout"),
            "expected connect-failure or timeout message, got: {msg}"
        );
    }

    // #583 acceptance tests: DOM discovery must honor max_depth.

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_max_depth_zero_returns_empty() {
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Seed page with 10 internal links — without the fix, all 10 would be
        // returned and crawled regardless of max_depth.
        let links: String = (1..=10)
            .map(|i| format!(r#"<a href="https://example.com/page/{i}">Link {i}</a>"#))
            .collect();
        let html = format!("<html><body><a href=\"/\">Home</a>{links}</body></html>");

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let seed = Url::parse(&server.uri()).unwrap();
        let config = CrawlerConfig::builder(seed).max_depth(0).build();

        let urls = discover_urls_single_fetch(&server.uri(), &config, &CorrelationId::new())
            .await
            .expect("discovery should succeed");

        assert!(
            urls.is_empty(),
            "max_depth=0 must return no discovered URLs, got {}",
            urls.len()
        );
    }

    #[tokio::test]
    #[cfg(not(miri))]
    async fn test_discover_urls_max_depth_one_returns_links() {
        let _entry_off = entry_off();
        use wiremock::matchers::path;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let seed_url = server.uri();
        // Links must share the seed's host to pass `is_internal_link`. The
        // external host must be filtered out.
        let html = format!(
            r#"<html><body>
            <a href="{seed_url}/page/1">Link 1</a>
            <a href="{seed_url}/page/2">Link 2</a>
            <a href="https://external.com/page">External</a>
        </body></html>"#
        );

        Mock::given(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_string(html))
            .mount(&server)
            .await;

        let seed = Url::parse(&seed_url).unwrap();
        let config = CrawlerConfig::builder(seed).max_depth(1).build();

        let urls = discover_urls_single_fetch(&seed_url, &config, &CorrelationId::new())
            .await
            .expect("discovery should succeed");

        // Internal links only (external.com filtered out by is_internal_link).
        assert_eq!(
            urls.len(),
            2,
            "max_depth=1 must return internal links from seed"
        );
    }
