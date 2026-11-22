use super::*;

fn make_scraped(title: &str, url: &str) -> ScrapedContent {
    ScrapedContent {
        title: title.to_string(),
        content: "body".to_string(),
        url: ValidUrl::parse(url).unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    }
}

#[test]
fn test_scraped_content_with_minimal_fields() {
    let content = ScrapedContent {
        title: "Test Article".to_string(),
        content: "Test content".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };

    assert_eq!(content.title, "Test Article");
    assert!(content.excerpt.is_none());
    assert!(content.assets.is_empty());
}

#[test]
fn test_scraped_content_with_all_fields() {
    let content = ScrapedContent {
        title: "Complete Article".to_string(),
        content: "Full content here".to_string(),
        url: ValidUrl::parse("https://example.com/article").unwrap(),
        excerpt: Some("A short excerpt".to_string()),
        author: Some("John Doe".to_string()),
        date: Some("2024-01-15".to_string()),
        html: Some("<html>test</html>".to_string()),
        assets: vec![DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "/tmp/img.png".to_string(),
            asset_type: "image".to_string(),
            size: 2048,
        }],
        correlation_id: None,
        quality_hint: None,
    };

    assert_eq!(content.author, Some("John Doe".to_string()));
    assert_eq!(content.assets.len(), 1);
    assert_eq!(content.assets[0].size, 2048);
}

#[test]
fn scraped_content_display_shows_title_and_url() {
    let s = make_scraped("My Article", "https://example.com/post");
    let output = format!("{s}");
    assert!(output.contains("My Article"));
    assert!(output.contains("https://example.com"));
}

#[test]
fn scraped_content_display_empty_title_shows_placeholder() {
    let s = make_scraped("", "https://example.com");
    let output = format!("{s}");
    assert!(output.contains("(untitled)"));
}

#[test]
fn scraped_content_equal_when_identical() {
    let a = make_scraped("Title", "https://example.com");
    let b = make_scraped("Title", "https://example.com");
    assert_eq!(a, b);
}

#[test]
fn scraped_content_not_equal_different_url() {
    let a = make_scraped("Title", "https://example.com/a");
    let b = make_scraped("Title", "https://example.com/b");
    assert_ne!(a, b);
}

#[test]
fn scraped_content_not_equal_different_content() {
    let mut a = make_scraped("Title", "https://example.com");
    a.content = "one".to_string();
    let mut b = make_scraped("Title", "https://example.com");
    b.content = "two".to_string();
    assert_ne!(a, b);
}

#[test]
fn scraped_content_debug_contains_key_fields() {
    let s = make_scraped("Debug Test", "https://example.com");
    let dbg = format!("{s:?}");
    assert!(dbg.contains("Debug Test"));
    assert!(dbg.contains("example.com"));
    assert!(dbg.contains("ScrapedContent"));
}

#[test]
fn test_scraped_content_serde_roundtrip() {
    let content = ScrapedContent {
        title: "Serde Article".to_string(),
        content: "Full body".to_string(),
        url: ValidUrl::parse("https://example.com/article").unwrap(),
        excerpt: Some("excerpt".to_string()),
        author: Some("Author".to_string()),
        date: Some("2024-06-15".to_string()),
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let json = serde_json::to_string(&content).unwrap();
    let deserialized: ScrapedContent = serde_json::from_str(&json).unwrap();
    assert_eq!(content.title, deserialized.title);
    assert_eq!(content.content, deserialized.content);
    assert_eq!(content.excerpt, deserialized.excerpt);
    assert_eq!(content.author, deserialized.author);
}

#[test]
fn test_scraped_content_serde_with_assets() {
    let content = ScrapedContent {
        title: "Assets".to_string(),
        content: "body".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: vec![DownloadedAsset {
            url: "https://example.com/img.png".to_string(),
            local_path: "/tmp/img.png".to_string(),
            asset_type: "image".to_string(),
            size: 1024,
        }],
        correlation_id: None,
        quality_hint: None,
    };
    let json = serde_json::to_string(&content).unwrap();
    let deserialized: ScrapedContent = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.assets.len(), 1);
    assert_eq!(deserialized.assets[0].size, 1024);
}

#[test]
fn test_scraped_content_serde_skips_none_html() {
    let content = make_scraped("Test", "https://example.com");
    let json = serde_json::to_string(&content).unwrap();
    assert!(!json.contains("html"));
}

#[test]
fn test_scraped_content_serde_includes_some_html() {
    let mut content = make_scraped("Test", "https://example.com");
    content.html = Some("<html>".to_string());
    let json = serde_json::to_string(&content).unwrap();
    assert!(json.contains("html"));
}

#[test]
fn test_scraped_content_clone() {
    let content = make_scraped("Clone Test", "https://example.com");
    let cloned = content.clone();
    assert_eq!(content, cloned);
}
