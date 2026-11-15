//! Tests de config/creación del Downloader.
//! Split desde `adapters/downloader/mod.rs` bajo la regla ~500 (#2584, Ola 1).

use super::*;
use crate::domain::config::ScraperConfig;
use tempfile::TempDir;

#[test]
fn issue_1117_hostile_urls_cannot_reach_download() {
    // The exact shapes that used to slip through to the network layer.
    assert!(
        ValidUrl::parse("data:text/html,<h1>hi</h1>").is_err(),
        "data: must be rejected before download() can be called"
    );
    assert!(
        ValidUrl::parse("file:///etc/passwd").is_err(),
        "file: must be rejected at the type boundary"
    );
    // Credentials are stripped by the type, never forwarded to the pool.
    let with_creds =
        ValidUrl::parse("https://user:secret@example.com/a.png").expect("https is valid");
    assert_eq!(
        with_creds.as_str(),
        "https://example.com/a.png",
        "the credential pair must not survive into the download URL"
    );
}
#[test]
fn test_to_download_config_maps_fields() {
    let scraper = ScraperConfig::default()
        .with_images()
        .with_download_concurrency(7)
        .with_asset_naming(AssetNamingStrategy::Slug);
    let dl = scraper.to_download_config();
    assert_eq!(dl.concurrency_limit, 7);
    assert_eq!(dl.asset_naming, AssetNamingStrategy::Slug);
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_downloader_creation() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        ..Default::default()
    };
    let downloader = Downloader::new(config);
    assert!(downloader.is_ok());
}

#[cfg_attr(miri, ignore = "btls-sys FFI (wreq Client) not supported by Miri")]
#[tokio::test]

async fn test_downloader_lazy_directories() {
    let temp_dir = TempDir::new().unwrap();
    let config = DownloadConfig {
        output_dir: temp_dir.path().to_path_buf(),
        images_dir: "test_images".to_string(),
        documents_dir: "test_docs".to_string(),
        ..Default::default()
    };

    // Directories are created lazily on the first actual download (issue
    // #606): when no assets are downloaded we must not litter the CWD with
    // empty `output/` + subdir trees.
    let downloader = Downloader::new(config).unwrap();

    let images_path = temp_dir.path().join("test_images");
    let docs_path = temp_dir.path().join("test_docs");

    assert!(
        !images_path.exists(),
        "Images directory should not be pre-created"
    );
    assert!(
        !docs_path.exists(),
        "Documents directory should not be pre-created"
    );

    // A real download creates the subdir on demand via ensure_subdir.
    downloader.ensure_subdir(&images_path).unwrap();
    assert!(
        images_path.exists(),
        "Images directory should be created lazily on first download"
    );
}

#[test]

fn test_downloader_config_concurrency() {
    let config = DownloadConfig {
        concurrency_limit: 10,
        ..Default::default()
    };
    assert_eq!(config.concurrency_limit, 10);
}

#[test]

fn test_downloader_config_user_agent() {
    let custom_ua = "MyCustomBot/1.0";
    let config = DownloadConfig {
        user_agent: custom_ua.to_string(),
        ..Default::default()
    };
    assert_eq!(config.user_agent, custom_ua);
}

#[test]

fn test_downloader_default_user_agent() {
    let config = DownloadConfig::default();
    assert!(
        config.user_agent.starts_with("WebCrawlerStaticPages/"),
        "Default user agent should include version"
    );
}
