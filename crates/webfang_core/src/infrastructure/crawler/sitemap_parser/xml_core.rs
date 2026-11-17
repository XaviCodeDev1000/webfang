use super::*;

/// Shared core parser for sitemap XML over an in-memory buffered body.
///
/// Extracts `<loc>`, `<lastmod>`, `<priority>`, and `<changefreq>` from `<url>`
/// entries. When `handle_sitemap_index` is true, also handles `<sitemap>`
/// entries in sitemap index files and returns `is_index`.
///
/// Uses `read_text_into` for robust `<loc>` content consumption (handles
/// mixed Text/CDATA/GeneralRef). When `url_validator` is provided, filters
/// invalid URL patterns per domain rules.
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
pub(super) fn parse_sitemap_core<R>(
    reader: &mut Reader<R>,
    base_url: &Url,
    handle_sitemap_index: bool,
    url_validator: Option<&UrlValidator>,
) -> Result<(Vec<SitemapUrl>, bool)>
where
    R: std::io::BufRead,
{
    let mut urls = Vec::new();
    let mut buf = Vec::new();
    let mut root_tag: Option<Vec<u8>> = None;
    let mut is_index = false;
    let mut saw_element = false;

    // State machine for metadata parsing
    let mut in_url = false;
    let mut in_sitemap = false;
    let mut in_loc = false;
    let mut in_lastmod = false;
    let mut in_priority = false;
    let mut in_changefreq = false;
    let mut current_url: Option<Url> = None;
    let mut current_lastmod: Option<String> = None;
    let mut current_priority: Option<f32> = None;
    let mut current_changefreq: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                saw_element = true;
                let name = e.name();

                // Bug 7: classify by root element (only when handling index)
                if handle_sitemap_index && root_tag.is_none() {
                    root_tag = match name.as_ref() {
                        b"sitemapindex" => {
                            is_index = true;
                            Some(name.as_ref().to_vec())
                        },
                        b"urlset" => Some(name.as_ref().to_vec()),
                        _ => return Err(SitemapError::InvalidStructure),
                    };
                }

                if name.as_ref() == b"url" {
                    in_url = true;
                    current_url = None;
                    current_lastmod = None;
                    current_priority = None;
                    current_changefreq = None;
                } else if handle_sitemap_index && name.as_ref() == b"sitemap" {
                    in_sitemap = true;
                    current_url = None;
                    current_lastmod = None;
                } else if (in_url || (handle_sitemap_index && in_sitemap))
                    && name.as_ref() == b"loc"
                {
                    in_loc = true;
                } else if in_url && name.as_ref() == b"lastmod" {
                    in_lastmod = true;
                } else if in_url && name.as_ref() == b"priority" {
                    in_priority = true;
                } else if in_url && name.as_ref() == b"changefreq" {
                    in_changefreq = true;
                }
            },
            Ok(Event::End(ref e)) => {
                let name = e.name();
                if name.as_ref() == b"url" {
                    in_url = false;
                    if let Some(ref url) = current_url {
                        urls.push(SitemapUrl {
                            url: url.clone(),
                            lastmod: current_lastmod.clone(),
                            priority: current_priority,
                            changefreq: current_changefreq.clone(),
                        });
                    }
                } else if handle_sitemap_index && name.as_ref() == b"sitemap" {
                    in_sitemap = false;
                    if let Some(ref url) = current_url {
                        urls.push(SitemapUrl {
                            url: url.clone(),
                            lastmod: current_lastmod.clone(),
                            priority: None,
                            changefreq: None,
                        });
                    }
                } else if name.as_ref() == b"loc" {
                    in_loc = false;
                } else if name.as_ref() == b"lastmod" {
                    in_lastmod = false;
                } else if name.as_ref() == b"priority" {
                    in_priority = false;
                } else if name.as_ref() == b"changefreq" {
                    in_changefreq = false;
                }
            },
            Ok(Event::Text(ref e)) if in_loc => {
                // Fallback for text content not captured by read_text_into
                let text = e
                    .decode()
                    .map_err(|e| SitemapError::XmlError(e.to_string()))?;
                let url_str = text.trim();
                if !url_str.is_empty() {
                    if let Some(url) = resolve_url(base_url, url_str) {
                        apply_url_validation(url, url_validator, &mut current_url);
                    }
                }
            },
            Ok(Event::CData(ref e)) if in_loc => {
                // Handle CDATA content in <loc> (e.g., <loc><![CDATA[https://example.com]]></loc>)
                let url_str = String::from_utf8_lossy(e).trim().to_string();
                if !url_str.is_empty() {
                    if let Some(url) = resolve_url(base_url, &url_str) {
                        apply_url_validation(url, url_validator, &mut current_url);
                    }
                }
            },
            Ok(Event::Text(ref e)) if in_lastmod => {
                if let Ok(text) = e.decode() {
                    current_lastmod = Some(text.trim().to_string());
                }
            },
            Ok(Event::Text(ref e)) if in_priority => {
                if let Ok(text) = e.decode() {
                    if let Ok(p) = text.trim().parse::<f32>() {
                        current_priority = Some(p.clamp(0.0, 1.0));
                    }
                }
            },
            Ok(Event::Text(ref e)) if in_changefreq => {
                if let Ok(text) = e.decode() {
                    current_changefreq = Some(text.trim().to_string());
                }
            },
            Ok(Event::Eof) => break,
            Err(e) => return Err(SitemapError::XmlError(e.to_string())),
            _ => {},
        }
        buf.clear();
    }

    // A body with no XML elements at all (e.g. plain text garbage) is NOT an
    // empty sitemap — it has no recognizable structure. Distinguishing this
    // from a valid-but-empty `<urlset/>` keeps AllChildrenFailed counts honest
    // (stabilization-sitemap-regression).
    if !saw_element {
        return Err(SitemapError::InvalidStructure);
    }

    Ok((urls, is_index))
}

/// Deduplicate and sort SitemapUrl entries for deterministic ordering.
///
/// 1. Sorts by URL and deduplicates (preserves first occurrence with metadata)
/// 2. Sorts by priority descending, then lastmod descending
pub(super) fn dedup_and_sort_sitemap_urls(urls: &mut Vec<SitemapUrl>) {
    // Deduplicate by URL (preserve first occurrence with metadata)
    urls.sort_by(|a, b| a.url.cmp(&b.url));
    urls.dedup_by(|a, b| a.url == b.url);

    // Sort for deterministic ordering: priority desc, then lastmod desc
    urls.sort_by(|a, b| {
        let pri_a = a.priority.unwrap_or(0.5);
        let pri_b = b.priority.unwrap_or(0.5);
        pri_b
            .partial_cmp(&pri_a)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.lastmod.cmp(&a.lastmod))
    });
}

/// Apply URL validation and update current_url if valid.
fn apply_url_validation(
    url: Url,
    url_validator: Option<&UrlValidator>,
    current_url: &mut Option<Url>,
) {
    if let Some(validator) = url_validator {
        let validation = validator.filter_invalid_patterns(&url);
        match validation {
            crate::domain::ValidationResult::Valid => {
                *current_url = Some(url);
            },
            crate::domain::ValidationResult::Invalid(reason) => {
                tracing::debug!("Filtered invalid URL: {} — {}", url, reason);
            },
            crate::domain::ValidationResult::NeedsRedirect(new_url) => {
                *current_url = Some(new_url);
            },
        }
    } else {
        *current_url = Some(url);
    }
}

/// Parse sitemap XML content using quick-xml (streaming parser)
///
/// Standalone streaming parser for sitemap entries with metadata,
/// extracted from the application layer (issue #442): XML parsing is an
/// infrastructure concern and lives here alongside [`SitemapParser`].
/// Relative URLs are resolved against `base_url`.
///
/// Following **xml-no-regex**: Uses quick-xml instead of regex for XML parsing.
/// Following **mem-stream-processing**: Streaming approach avoids loading entire DOM.
///
/// # Arguments
///
/// * `xml_content` - XML content of the sitemap
/// * `base_url` - Base URL to resolve relative entries against
///
/// # Returns
///
/// * `Ok(Vec<SitemapUrl>)` - List of URLs with metadata (empty if no URLs found)
/// * `Err(CrawlError)` - Parse error
#[allow(clippy::too_many_lines)]
pub fn parse_sitemap(
    xml_content: &str,
    base_url: &Url,
) -> std::result::Result<Vec<SitemapUrl>, CrawlError> {
    let mut reader = Reader::from_str(xml_content);
    let (mut urls, _is_index) = parse_sitemap_core(&mut reader, base_url, false, None)
        .map_err(|e| CrawlError::Parse(e.to_string()))?;

    // Lenient: return empty vec for empty sitemaps (matches old behavior for tests/benchmarks)
    if urls.is_empty() {
        return Ok(Vec::new());
    }
    dedup_and_sort_sitemap_urls(&mut urls);

    Ok(urls)
}
