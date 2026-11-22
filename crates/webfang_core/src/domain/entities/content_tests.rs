use super::*;

use crate::domain::ValidUrl;

#[test]
fn test_document_chunk_from_scraped_content() {
    let scraped = ScrapedContent {
        title: "Test Title".to_string(),
        content: "Test content body".to_string(),
        url: ValidUrl::parse("https://example.com/article").unwrap(),
        excerpt: Some("An excerpt".to_string()),
        author: Some("Jane Doe".to_string()),
        date: Some("2024-06-01".to_string()),
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };

    let chunk = DocumentChunk::from_scraped_content(&scraped);

    assert_eq!(chunk.url, "https://example.com/article");
    assert_eq!(chunk.title, "Test Title");
    assert_eq!(chunk.content, "Test content body");
    assert!(chunk.metadata.contains_key("excerpt"));
    assert!(chunk.metadata.contains_key("author"));
    assert!(chunk.metadata.contains_key("date"));
    assert!(chunk.metadata.contains_key("domain"));
    assert_eq!(chunk.metadata["domain"], "example.com");
    assert!(chunk.embeddings.is_none());
}

#[test]
fn test_document_chunk_validate_empty_content() {
    let chunk = DocumentChunk::new(uuid::Uuid::new_v4(), "https://example.com", "Title", "");
    assert!(matches!(
        chunk.validate(),
        Err(ValidationError::EmptyContent)
    ));
}

#[test]
fn test_document_chunk_validate_empty_title() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "",
        "content here",
    );
    assert!(matches!(chunk.validate(), Err(ValidationError::EmptyTitle)));
}

#[test]
fn test_document_chunk_validate_invalid_url() {
    let chunk = DocumentChunk::new(uuid::Uuid::new_v4(), "not-a-url", "Title", "content here");
    assert!(matches!(
        chunk.validate(),
        Err(ValidationError::InvalidUrl(_))
    ));
}

#[test]
fn test_document_chunk_has_embeddings_true() {
    let chunk = DocumentChunk::test_new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
    )
    .with_embeddings(vec![0.1, 0.2, 0.3]);
    assert!(chunk.has_embeddings());
}

#[test]
fn test_document_chunk_has_embeddings_false() {
    let chunk = DocumentChunk::test_new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
    );
    assert!(!chunk.has_embeddings());
}

#[test]
fn test_document_chunk_text_length_nonempty() {
    let chunk = DocumentChunk::test_new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "hello world",
    );
    assert_eq!(chunk.text_length(), 11);
}

#[test]
fn test_document_chunk_text_length_empty() {
    let chunk = DocumentChunk::test_new(uuid::Uuid::new_v4(), "https://example.com", "Title", "");
    assert_eq!(chunk.text_length(), 0);
}

#[test]
fn test_document_chunk_with_metadata() {
    let mut meta = HashMap::new();
    meta.insert("author".to_string(), "Alice".to_string());
    meta.insert("source".to_string(), "rss".to_string());

    let chunk = DocumentChunk::with_metadata(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
        meta,
    );

    assert_eq!(chunk.metadata.len(), 2);
    assert_eq!(chunk.metadata["author"], "Alice");
    assert_eq!(chunk.metadata["source"], "rss");
}

#[test]
fn test_document_chunk_with_id() {
    let id = uuid::Uuid::new_v4();
    let scraped = ScrapedContent {
        title: "Test".to_string(),
        content: "Content".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = DocumentChunk::with_id(&scraped, id);
    assert_eq!(chunk.id, id);
}

#[test]
fn test_document_chunk_from_scraped_content_with_correlation() {
    let corr = CorrelationId::new();
    let scraped = ScrapedContent {
        title: "With Corr".to_string(),
        content: "Body".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = DocumentChunk::from_scraped_content_with_correlation(&scraped, Some(&corr));
    assert!(chunk.correlation_id.is_some());
    assert!(chunk.correlation_id.unwrap().starts_with("00-"));
}

#[test]
fn test_document_chunk_from_scraped_content_no_correlation() {
    let scraped = ScrapedContent {
        title: "No Corr".to_string(),
        content: "Body".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = DocumentChunk::from_scraped_content(&scraped);
    assert!(chunk.correlation_id.is_none());
}

#[test]
fn test_document_chunk_from_scraped_content_metadata_extraction() {
    let scraped = ScrapedContent {
        title: "Meta Test".to_string(),
        content: "Body".to_string(),
        url: ValidUrl::parse("https://blog.example.com/post").unwrap(),
        excerpt: Some("excerpt text".to_string()),
        author: Some("Author".to_string()),
        date: Some("2024-01-01".to_string()),
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = DocumentChunk::from_scraped_content(&scraped);
    assert_eq!(chunk.metadata["excerpt"], "excerpt text");
    assert_eq!(chunk.metadata["author"], "Author");
    assert_eq!(chunk.metadata["date"], "2024-01-01");
    assert_eq!(chunk.metadata["domain"], "blog.example.com");
}

#[test]
fn test_document_chunk_validate_success() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Valid Title",
        "Valid content",
    );
    let validated = chunk.validate();
    assert!(validated.is_ok());
}

#[test]
fn test_document_chunk_validate_whitespace_only_content() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "   \t\n  ",
    );
    assert!(matches!(
        chunk.validate(),
        Err(ValidationError::EmptyContent)
    ));
}

#[test]
fn test_document_chunk_validate_whitespace_only_title() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "   ",
        "content",
    );
    assert!(matches!(chunk.validate(), Err(ValidationError::EmptyTitle)));
}

#[test]
fn test_document_chunk_validate_empty_metadata_value() {
    let mut meta = HashMap::new();
    meta.insert("key".to_string(), "   ".to_string());

    let chunk = DocumentChunk::with_metadata(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
        meta,
    );
    assert!(matches!(
        chunk.validate(),
        Err(ValidationError::InvalidMetadata(_))
    ));
}

#[test]
fn test_document_chunk_validate_preserves_id() {
    let id = uuid::Uuid::new_v4();
    let chunk = DocumentChunk::new(id, "https://example.com", "Title", "content");
    let validated = chunk.validate().unwrap();
    assert_eq!(validated.id, id);
}

#[test]
fn test_document_chunk_validate_preserves_metadata() {
    let mut meta = HashMap::new();
    meta.insert("author".to_string(), "Test".to_string());
    let chunk = DocumentChunk::with_metadata(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
        meta,
    );
    let validated = chunk.validate().unwrap();
    assert_eq!(validated.metadata["author"], "Test");
}

#[test]
fn test_document_chunk_serde_roundtrip() {
    let chunk = DocumentChunk::new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Serde Test",
        "Some content",
    );
    let json = serde_json::to_string(&chunk).unwrap();
    let deserialized: DocumentChunk<Draft> = serde_json::from_str(&json).unwrap();
    assert_eq!(chunk.url, deserialized.url);
    assert_eq!(chunk.title, deserialized.title);
    assert_eq!(chunk.content, deserialized.content);
}

#[test]
fn test_document_chunk_with_embeddings_serde_roundtrip() {
    let chunk = DocumentChunk::test_new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
    )
    .with_embeddings(vec![0.1, 0.2, 0.3]);
    let json = serde_json::to_string(&chunk).unwrap();
    assert!(json.contains("0.1"));
    let deserialized: DocumentChunk<Draft> = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.embeddings, Some(vec![0.1, 0.2, 0.3]));
}

#[test]
fn test_document_chunk_from_scraped_content_no_optional_fields() {
    let scraped = ScrapedContent {
        title: "Minimal".to_string(),
        content: "content".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };
    let chunk = DocumentChunk::from_scraped_content(&scraped);
    assert_eq!(chunk.metadata.len(), 1); // only domain
    assert!(chunk.metadata.contains_key("domain"));
}

#[test]
fn test_document_chunk_with_embeddings_chain() {
    let chunk = DocumentChunk::test_new(
        uuid::Uuid::new_v4(),
        "https://example.com",
        "Title",
        "content",
    )
    .with_embeddings(vec![1.0])
    .with_embeddings(vec![1.0, 2.0]); // second call replaces

    assert_eq!(chunk.embeddings.unwrap(), vec![1.0, 2.0]);
}

#[test]
fn test_validation_error_display() {
    assert_eq!(ValidationError::EmptyContent.to_string(), "empty_content");
    assert_eq!(ValidationError::EmptyTitle.to_string(), "empty_title");
    assert!(ValidationError::InvalidUrl("bad".to_string())
        .to_string()
        .contains("bad"));
    assert!(ValidationError::InvalidMetadata("reason".to_string())
        .to_string()
        .contains("reason"));
}

// ---------------------------------------------------------------------------
// Regression tests for #569: AI chunks must inherit url/title from scraped page
// ---------------------------------------------------------------------------

/// `enrich_from_scraped_content` must fill empty url/title so validate() passes.
#[test]
fn test_enrich_from_scraped_content_fills_empty_fields() {
    // Simulate an AI-generated chunk: has content + embeddings but no url/title.
    let ai_chunk = DocumentChunk::new(
        Uuid::new_v4(),
        "", // empty url (from HtmlChunker)
        "", // empty title (from HtmlChunker)
        "AI-cleaned semantic content",
    )
    .with_embeddings(vec![0.1, 0.2, 0.3]);

    let scraped = ScrapedContent {
        title: "Source Page Title".to_string(),
        content: "raw page content".to_string(),
        url: ValidUrl::parse("https://example.com/article").unwrap(),
        excerpt: Some("An excerpt".to_string()),
        author: Some("Author Name".to_string()),
        date: Some("2024-01-15".to_string()),
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };

    let enriched = ai_chunk.enrich_from_scraped_content(&scraped);

    assert_eq!(enriched.url, "https://example.com/article");
    assert_eq!(enriched.title, "Source Page Title");
    assert_eq!(enriched.content, "AI-cleaned semantic content");
    assert_eq!(enriched.embeddings, Some(vec![0.1, 0.2, 0.3]));
    // #800: page-level author/excerpt must NOT leak into the AI chunk.
    // Absent is truthful — per-chunk attribution is unknowable without ML.
    assert!(
        !enriched.metadata.contains_key("excerpt"),
        "AI chunk must NOT inherit the page excerpt"
    );
    assert!(
        !enriched.metadata.contains_key("author"),
        "AI chunk must NOT inherit the page author"
    );
    assert_eq!(enriched.metadata["date"], "2024-01-15");
    assert_eq!(enriched.metadata["domain"], "example.com");

    // The enriched chunk must still pass validation (the core of #569):
    // url/title/date/domain are enough for a valid chunk.
    assert!(
        enriched.validate().is_ok(),
        "enriched AI chunk must pass validation"
    );
}

/// Enrichment must NOT overwrite existing metadata from the AI side.
#[test]
fn test_enrich_from_scraped_content_preserves_existing_metadata() {
    let mut meta = HashMap::new();
    meta.insert("author".to_string(), "AI-Inferred-Author".to_string());
    meta.insert("custom_key".to_string(), "custom_value".to_string());

    let ai_chunk = DocumentChunk::with_metadata(Uuid::new_v4(), "", "", "content", meta);

    let scraped = ScrapedContent {
        title: "Title".to_string(),
        content: "content".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: Some("Scraped-Author".to_string()),
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };

    let enriched = ai_chunk.enrich_from_scraped_content(&scraped);

    // AI-side author is preserved, scraped author is NOT inserted.
    assert_eq!(enriched.metadata["author"], "AI-Inferred-Author");
    // Custom AI-side key is preserved.
    assert_eq!(enriched.metadata["custom_key"], "custom_value");
    // Domain is inserted (was missing).
    assert_eq!(enriched.metadata["domain"], "example.com");
}

/// Enrichment with minimal scraped content (no optional fields) still works.
#[test]
fn test_enrich_from_scraped_content_minimal_scraped() {
    let ai_chunk = DocumentChunk::new(Uuid::new_v4(), "", "", "AI content");

    let scraped = ScrapedContent {
        title: "Minimal".to_string(),
        content: "raw".to_string(),
        url: ValidUrl::parse("https://example.com").unwrap(),
        excerpt: None,
        author: None,
        date: None,
        html: None,
        assets: Vec::new(),
        correlation_id: None,
        quality_hint: None,
    };

    let enriched = ai_chunk.enrich_from_scraped_content(&scraped);

    assert_eq!(enriched.url, "https://example.com/");
    assert_eq!(enriched.title, "Minimal");
    assert_eq!(enriched.metadata.len(), 1); // only domain
    assert!(enriched.validate().is_ok());
}
