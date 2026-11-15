//! Tests de naming/filtros (filename, slug, content-disposition, filters).
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use super::tests_common::vu;
use super::*;
use tempfile::TempDir;

#[test]
fn test_mime_type_to_extension() {
    assert_eq!(mime_type_to_extension("image/png"), Some("png".to_string()));
    assert_eq!(
        mime_type_to_extension("image/jpeg"),
        Some("jpg".to_string())
    );
    assert_eq!(
        mime_type_to_extension("application/pdf"),
        Some("pdf".to_string())
    );
    assert_eq!(mime_type_to_extension("application/unknown"), None);
    assert_eq!(mime_type_to_extension(""), None);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]

fn test_generate_filename_hash_strategy() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();

    let filename = downloader.generate_filename(
        &vu("https://example.com/img.png"),
        "abc123def456789",
        Some("image/png"),
        None,
    );
    assert!(
        filename.ends_with(".png"),
        "Expected .png but got: {filename}"
    );
    assert!(
        filename.starts_with("abc123def456"),
        "Filename should start with first 12 chars of hash"
    );

    let filename = downloader.generate_filename(
        &vu("https://example.com/file"),
        "xyz789abc123456",
        None,
        None,
    );
    assert!(
        filename.ends_with(".bin"),
        "Expected .bin but got: {filename}"
    );
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]

fn test_generate_filename_slug_strategy() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        asset_naming: AssetNamingStrategy::Slug,
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();

    let filename = downloader.generate_filename(
        &vu("https://example.com/docs/rust-book.pdf"),
        "abc123def456789",
        Some("application/pdf"),
        None,
    );
    assert_eq!(filename, "rust-book.pdf");
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]

fn test_generate_filename_content_disposition() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        asset_naming: AssetNamingStrategy::ContentDisposition,
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();

    let filename = downloader.generate_filename(
        &vu("https://example.com/download"),
        "abc123def456789",
        Some("application/pdf"),
        Some("annual-report.pdf"),
    );
    assert_eq!(filename, "annual-report.pdf");
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]

fn test_generate_filename_content_disposition_fallback() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        asset_naming: AssetNamingStrategy::ContentDisposition,
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();

    // No Content-Disposition → falls back to hash
    let filename = downloader.generate_filename(
        &vu("https://example.com/download"),
        "abc123def456789",
        Some("application/pdf"),
        None,
    );
    assert!(filename.starts_with("abc123def456"));
}

#[test]

fn test_url_matches_filters_empty_includes() {
    assert!(url_matches_filters(
        &vu("https://example.com/file.pdf"),
        &[],
        &[]
    ));
}

#[test]

fn test_url_matches_filters_exclude_wins() {
    let excludes = vec!["/*.pdf".to_string()];
    assert!(!url_matches_filters(
        &vu("https://example.com/file.pdf"),
        &[],
        &excludes
    ));
}

#[test]

fn test_url_matches_filters_include_only() {
    let includes = vec!["/*.pdf".to_string()];
    assert!(url_matches_filters(
        &vu("https://example.com/file.pdf"),
        &includes,
        &[]
    ));
    assert!(!url_matches_filters(
        &vu("https://example.com/file.jpg"),
        &includes,
        &[]
    ));
}

#[test]

fn test_url_matches_filters_extension_glob() {
    let includes = vec!["*.pdf".to_string()];
    assert!(url_matches_filters(
        &vu("https://x.com/file.pdf"),
        &includes,
        &[]
    ));
    assert!(!url_matches_filters(
        &vu("https://x.com/file.jpg"),
        &includes,
        &[]
    ));
}

#[test]

fn test_sanitize_filename() {
    // Delegates to `binary_utils::sanitize_filename_component`: only
    // the last valid segment survives.
    assert_eq!(sanitize_filename("hello/world"), "world");
    assert_eq!(sanitize_filename("file\0name"), "filename");
    assert_eq!(sanitize_filename("normal-file.pdf"), "normal-file.pdf");
}

// -------------------------------------------------------------
// sanitize_filename — traversal hardening (batch 1, unified in
// binary_utils::sanitize_filename_component since batch 3)
// -------------------------------------------------------------

#[test]

fn sanitize_filename_strips_traversal_separators() {
    // Directory components are dropped entirely; the last valid
    // segment cannot traverse on join.
    assert_eq!(sanitize_filename("../escape.bin"), "escape.bin");
    assert_eq!(sanitize_filename("a/../b"), "b");
    assert_eq!(sanitize_filename("/abs/path.bin"), "path.bin");
    assert_eq!(sanitize_filename(r"\\server\share"), "share");
}

#[test]

fn sanitize_filename_neutralizes_bare_dot_segments() {
    // `.` / `..` joined onto a directory resolve outside it → empty
    // result so generate_filename falls back to hash-based naming.
    assert_eq!(sanitize_filename(".."), "");
    assert_eq!(sanitize_filename("."), "");
    assert_eq!(sanitize_filename("..\\"), "");
}

#[test]

fn sanitize_filename_removes_null_bytes() {
    assert_eq!(sanitize_filename("nul\0byte.pdf"), "nulbyte.pdf");
}

#[test]

fn sanitize_filename_caps_length() {
    let long = format!("{}.pdf", "a".repeat(300));
    let sanitized = sanitize_filename(&long);
    // ext4 per-component cap; matches binary_utils::MAX_FILENAME_LEN.
    assert!(sanitized.len() <= 255);
    assert!(!sanitized.is_empty());
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[test]

fn generate_filename_cd_falls_back_to_hash_when_fully_hostile() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        asset_naming: AssetNamingStrategy::ContentDisposition,
        ..Default::default()
    };
    let downloader = Downloader::new(config).unwrap();

    // Bare `..` sanitizes to empty → hash-based fallback.
    let filename = downloader.generate_filename(
        &vu("https://example.com/download"),
        "abc123def456789",
        Some("application/pdf"),
        Some(".."),
    );
    assert!(filename.starts_with("abc123def456"));
    assert!(filename.ends_with(".pdf"));
}

#[test]

fn test_parse_content_disposition_quoted() {
    let val = r#"attachment; filename="report.pdf""#;
    assert_eq!(
        parse_content_disposition_header(val),
        Some("report.pdf".to_string())
    );
}

#[test]

fn test_parse_content_disposition_unquoted() {
    let val = "attachment; filename=report.pdf";
    assert_eq!(
        parse_content_disposition_header(val),
        Some("report.pdf".to_string())
    );
}

#[test]

fn test_parse_content_disposition_missing() {
    assert_eq!(parse_content_disposition_header("attachment"), None);
}

#[test]

fn test_derive_slug_from_url() {
    assert_eq!(
        derive_slug_from_url(&vu("https://example.com/docs/book.pdf")),
        "book.pdf"
    );
    assert_eq!(derive_slug_from_url(&vu("https://example.com/")), "");
}
