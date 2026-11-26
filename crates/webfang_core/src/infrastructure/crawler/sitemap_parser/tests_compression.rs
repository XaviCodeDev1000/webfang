use super::*;
use webfang_test_utils::EnvGuard;

/// Compress `data` with gzip using the existing workspace dependency
/// (`async-compression` only exposes `tokio::bufread` adapters).
async fn gzip_compress(data: &[u8]) -> Vec<u8> {
    use async_compression::tokio::bufread::GzipEncoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let mut encoder = GzipEncoder::new(BufReader::new(std::io::Cursor::new(data)));
    let mut out = Vec::new();
    encoder.read_to_end(&mut out).await.unwrap();
    out
}

/// #757 regression — the MDN case: a server serves an `.xml.gz` sitemap
/// whose body is gzip AND sets `content-encoding: gzip` for transport.
/// The HTTP client (`wreq` built with `.gzip(true)`) auto-decompresses the
/// transport layer and strips `Content-Encoding`, delivering plain XML.
/// Before the fix, `CompressionHandler` trusted the `.gz` extension and
/// decompressed again -> "decompression failed: Invalid gzip header".
/// After the fix, extension without magic bytes passes through.
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn test_sitemap_gz_with_content_encoding_header() {
    let _entry_off = EnvGuard::entry_guard_off();
    use wiremock::{MockServer, ResponseTemplate};

    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://developer.mozilla.org/en-US/docs/page1</loc></url>
        <url><loc>https://developer.mozilla.org/en-US/docs/page2</loc></url>
    </urlset>"#;

    let urls = test_support::serve_and_parse(
        &mock,
        "/sitemap.xml.gz",
        ResponseTemplate::new(200)
            .set_body_raw(gzip_compress(xml.as_bytes()).await, "application/gzip")
            .insert_header("content-encoding", "gzip"),
        "/sitemap.xml.gz",
    )
    .await
    .expect("body gzip + content-encoding gzip must parse (#757)");
    assert_eq!(urls.len(), 2);
}

/// #757 case (b): a gzip body behind a `.gz` URL WITHOUT a transport
/// `content-encoding` header. Today this works; it must keep working —
/// magic-byte sniffing detects gzip and the handler decompresses.
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn test_sitemap_gz_body_without_content_encoding() {
    let _entry_off = EnvGuard::entry_guard_off();
    use wiremock::{MockServer, ResponseTemplate};

    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/page1</loc></url>
    </urlset>"#;

    let urls = test_support::serve_and_parse(
        &mock,
        "/manual.xml.gz",
        ResponseTemplate::new(200)
            .set_body_raw(gzip_compress(xml.as_bytes()).await, "application/gzip"),
        "/manual.xml.gz",
    )
    .await
    .expect("gzip body without content-encoding must still decompress");
    assert_eq!(urls.len(), 1);
}

/// #757 case (c): a plain (already-decoded) body behind a lying `.gz` URL
/// with no compression headers. Before the fix this failed with
/// "Invalid gzip header" because the extension forced decompression;
/// after the fix, magic-byte sniffing passes it through untouched.
#[cfg_attr(miri, ignore = "wreq/btls TLS_method FFI (unsupported by Miri)")]
#[tokio::test]
async fn test_sitemap_lying_gz_extension_with_plain_body() {
    let _entry_off = EnvGuard::entry_guard_off();
    use wiremock::{MockServer, ResponseTemplate};

    let mock = MockServer::start().await;

    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/page1</loc></url>
    </urlset>"#;

    let urls = test_support::serve_and_parse(
        &mock,
        "/sitemap.xml.gz",
        ResponseTemplate::new(200).set_body_string(xml),
        "/sitemap.xml.gz",
    )
    .await
    .expect("plain body behind .gz URL must pass through (#757)");
    assert_eq!(urls.len(), 1);
}
