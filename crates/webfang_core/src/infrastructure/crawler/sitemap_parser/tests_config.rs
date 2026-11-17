use super::*;

#[test]
fn test_config_builder() {
    let config = SitemapConfig::builder()
        .gzip_enabled(true)
        .max_depth(5)
        .concurrency(10)
        .build();

    assert!(config.gzip_enabled);
    assert_eq!(config.max_depth, 5);
    assert_eq!(config.concurrency, 10);
}

#[test]
fn test_config_default() {
    let config = SitemapConfig::default();

    assert!(config.gzip_enabled);
    assert_eq!(config.max_depth, 3);
    assert_eq!(config.concurrency, 5);
}

#[test]
fn test_parser_has_gzip() {
    let parser_gzip =
        SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(true).build()).unwrap();
    assert!(parser_gzip.has_gzip());

    let parser_no_gzip =
        SitemapParser::with_config(SitemapConfig::builder().gzip_enabled(false).build()).unwrap();
    assert!(!parser_no_gzip.has_gzip());
}

#[test]
fn test_parse_sitemap_max_depth_exceeded() {
    // The `SitemapError::MaxDepthExceeded` Display assertion moved with the
    // error type to `domain::crawler_port::sitemap::tests` (ADR-0012-B);
    // the config-side invariant (depth 0 is representable) stays here.
    let config = SitemapConfig::builder().max_depth(0).build();
    assert_eq!(config.max_depth, 0);
}

#[test]
fn test_config_builder_zero_falls_back_to_defaults() {
    let config = SitemapConfig::builder()
        .max_response_size(0)
        .max_decompressed_size(0)
        .build();

    assert_eq!(config.max_response_size, 52_428_800);
    assert_eq!(config.max_decompressed_size, 104_857_600);
}

#[test]
fn test_max_depth_accessor() {
    let parser = SitemapParser::with_config(SitemapConfig::builder().max_depth(7).build()).unwrap();
    assert_eq!(parser.max_depth(), 7);
}

#[test]
fn test_max_depth_default() {
    let parser = SitemapParser::new().unwrap();
    assert_eq!(parser.max_depth(), 3);
}

// -- #323: tls_emulation is honored, not hardcoded --

#[test]
fn test_new_defaults_to_chrome145_profile() {
    let parser = SitemapParser::new().unwrap();
    assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
}

#[test]
fn test_with_config_defaults_to_chrome145_profile() {
    let parser = SitemapParser::with_config(SitemapConfig::default()).unwrap();
    assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome145);
}

#[test]
fn test_with_config_and_profile_accepts_custom_profile() {
    let parser = SitemapParser::with_config_and_profile(
        SitemapConfig::default(),
        wreq_util::Profile::Chrome131,
    )
    .unwrap();
    assert_eq!(parser.tls_emulation(), wreq_util::Profile::Chrome131);
}

#[test]
fn test_with_config_and_profile_accepts_firefox_profile() {
    let parser = SitemapParser::with_config_and_profile(
        SitemapConfig::builder().max_depth(2).build(),
        wreq_util::Profile::Firefox135,
    )
    .unwrap();
    assert_eq!(parser.tls_emulation(), wreq_util::Profile::Firefox135);
    assert_eq!(parser.max_depth(), 2);
}
