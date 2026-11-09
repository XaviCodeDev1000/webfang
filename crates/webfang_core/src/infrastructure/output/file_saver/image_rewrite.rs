//! Rewrite absolute image URLs in converted markdown to relative paths.

use std::path::Path;

/// Rewrite absolute image URLs in markdown to relative paths.
///
/// Scans for `![alt](https://...)` patterns and rewrites to `![alt](./relative/path)`.
/// Used as a fallback when `--download-images` is not enabled and `item.assets` is empty.
pub(super) fn rewrite_image_urls_to_relative(content: &str, _md_file_dir: &Path) -> String {
    use regex::Regex;

    // Match ![alt text](url) where url is absolute
    // Static regex is a compile-time constant; parsing cannot fail.
    #[allow(clippy::expect_used)]
    static RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"!\[([^\]]*)\]\((https?://[^)]+)\)")
            // LCOV_EXCL_LINE defensive: compile-time-regex — static pattern is a constant, parse cannot fail
            .expect("static image-markdown regex is valid")
    });

    RE.replace_all(content, |caps: &regex::Captures| {
        let alt = &caps[1];
        let url = &caps[2];

        // Parse the URL to extract the path
        if let Ok(parsed) = url::Url::parse(url) {
            let path = parsed.path();
            // Create a relative path from md_file_dir to the image
            // For simplicity, use the last 2 segments as the relative path
            let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            let rel_path = if segments.len() >= 2 {
                format!(
                    "./{}/{}",
                    segments[segments.len() - 2],
                    segments[segments.len() - 1]
                )
            } else if segments.len() == 1 {
                format!("./{}", segments[0])
            } else {
                url.to_string()
            };
            format!("![{alt}]({rel_path})")
        } else {
            caps[0].to_string()
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::rewrite_image_urls_to_relative;
    use std::path::Path;

    #[test]
    fn test_rewrite_single_image_url_to_relative() {
        let content = "![alt](https://example.com/img/photo.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./img/photo.png)");
    }

    #[test]
    fn test_rewrite_deeply_nested_image_url() {
        let content = "![alt](https://example.com/a/b/c.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./b/c.png)");
    }

    #[test]
    fn test_rewrite_non_url_images_unchanged() {
        let content = "![alt](./local/image.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./local/image.png)");
    }

    #[test]
    fn test_rewrite_empty_content_returns_empty() {
        let content = "";
        let dir = Path::new("/output");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "");
    }

    #[test]
    fn test_rewrite_multiple_images() {
        let content = "First ![a](https://example.com/x.png) then ![b](https://example.com/y.jpg)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "First ![a](./x.png) then ![b](./y.jpg)");
    }

    #[test]
    fn test_rewrite_single_segment_path() {
        let content = "![alt](https://example.com/photo.png)";
        let dir = Path::new("/output/example.com");
        let result = rewrite_image_urls_to_relative(content, dir);
        assert_eq!(result, "![alt](./photo.png)");
    }
}
