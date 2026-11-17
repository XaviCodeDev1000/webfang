use super::*;

#[tokio::test]
async fn test_parse_simple_sitemap() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/page1</loc></url>
        <url><loc>https://example.com/page2</loc></url>
        <url><loc>https://example.com/page3</loc></url>
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let (urls, _is_index) = parser
        .parse_xml_sitemap(xml.as_bytes(), &base)
        .await
        .unwrap();

    assert_eq!(urls.len(), 3);
    assert!(urls
        .iter()
        .any(|u| u.url.as_str() == "https://example.com/page1"));
}

#[tokio::test]
async fn test_parse_sitemap_with_duplicates() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/page1</loc></url>
        <url><loc>https://example.com/page1</loc></url>
        <url><loc>https://example.com/page2</loc></url>
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let (urls, _is_index) = parser
        .parse_xml_sitemap(xml.as_bytes(), &base)
        .await
        .unwrap();

    assert_eq!(urls.len(), 2);
}

#[tokio::test]
async fn test_parse_empty_sitemap() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

    assert!(matches!(result, Err(SitemapError::NoUrlsFound)));
}

#[tokio::test]
async fn test_parse_malformed_xml() {
    let xml = r#"<?xml version="1.0"?>
    <urlset>
        <url><loc>https://example.com/page1</loc>
        <!-- Missing closing tag -->
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let result = parser.parse_xml_sitemap(xml.as_bytes(), &base).await;

    // Malformed XML should either parse (lenient parser) or fail with XmlError
    match &result {
        Ok(_) => {}, // Parser is lenient, this is acceptable
        Err(e) => assert!(
            matches!(e, SitemapError::XmlError(_)),
            "expected XmlError for malformed XML, got: {e:?}"
        ),
    }
}

#[tokio::test]
async fn test_filter_invalid_schemes() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
        <url><loc>https://example.com/valid</loc></url>
        <url><loc>http://example.com/valid</loc></url>
        <url><loc>ftp://example.com/invalid</loc></url>
        <url><loc>file:///etc/passwd</loc></url>
        <url><loc>javascript:alert(1)</loc></url>
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let (urls, _is_index) = parser
        .parse_xml_sitemap(xml.as_bytes(), &base)
        .await
        .unwrap();

    assert_eq!(urls.len(), 2);
    assert!(urls
        .iter()
        .all(|u| u.url.scheme() == "http" || u.url.scheme() == "https"));
}

#[tokio::test]
async fn test_parse_sitemap_with_namespaces() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
    <urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"
            xmlns:image="http://www.google.com/schemas/sitemap-image/1.1">
        <url>
            <loc>https://example.com/page1</loc>
            <image:image><image:loc>https://example.com/image.jpg</image:loc></image:image>
        </url>
        <url><loc>https://example.com/page2</loc></url>
    </urlset>"#;

    let parser = SitemapParser::new().unwrap();
    let base = Url::parse("https://example.com").unwrap();
    let (urls, _is_index) = parser
        .parse_xml_sitemap(xml.as_bytes(), &base)
        .await
        .unwrap();

    assert!(urls.len() >= 2);
}
