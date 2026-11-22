//! Sitemap URL discovery probing — robots.txt, fallback paths, sub-path probes.
//!
//! Extracted from `sitemap_discovery.rs` (Ola 3 B5): the "where is the
//! sitemap" chain is a self-contained HTTP probing state machine with its own
//! HEAD→GET retry policy (stabilization-sitemap-regression, scenario 9),
//! independent of the crawl orchestration that consumes it. Pure move — no
//! behavior change.

use crate::domain::error::WafDetectionKind;
use crate::domain::waf::{waf_inspector, InspectionContext};
use crate::domain::CrawlError;
use url::Url;

/// Auto-discover sitemap URL from robots.txt or fallback locations.
///
/// Resolution order (each step short-circuits on first hit):
/// 1. Parse the `Sitemap:` directive from `robots.txt` (resolved against `base`).
/// 2. HEAD-probe common fallback paths (`/sitemap.xml`, ...).
/// 3. HEAD-probe nested sub-path sitemaps for sectioned sites (GAP 5 / Bug #30).
///
/// Following **own-borrow-over-clone**: Accepts `&str`.
/// Following **security-no-unwrap-in-prod**: Proper error handling.
///
/// # Arguments
///
/// * `base_url` - Base URL of the website
///
/// # Returns
///
/// * `Ok(String)` - Discovered sitemap URL
/// * `Err(CrawlError)` - Error during discovery
pub(super) async fn discover_sitemap_url(
    base_url: &str,
    client: &wreq::Client,
) -> Result<String, CrawlError> {
    let base = Url::parse(base_url).map_err(|e| CrawlError::InvalidUrl(e.to_string()))?;

    // 1. robots.txt `Sitemap:` directive (a WAF challenge aborts via `?`)
    if let Some(url) = fetch_robots_sitemap(&base, client).await? {
        return Ok(url);
    }
    tracing::debug!("No sitemap found in robots.txt, trying fallback locations");

    // 2. Common fallback locations
    const FALLBACK_PATHS: &[&str] = &[
        "/sitemap.xml",
        "/sitemap_index.xml",
        "/sitemap.xml.gz",
        "/sitemap/sitemap.xml",
    ];
    if let Some(url) = probe_sitemap_paths(&base, client, FALLBACK_PATHS).await {
        return Ok(url);
    }

    // 3. Nested sub-path sitemaps for sectioned sites
    if let Some(url) = probe_subpath_sitemaps(&base, client).await {
        return Ok(url);
    }

    tracing::warn!(base_url = %base_url, "no sitemap found");
    Err(CrawlError::SitemapNotFound(base_url.to_string()))
}

/// Fetch `robots.txt` and extract its `Sitemap:` directive, if present.
///
/// The response body is inspected for WAF challenges before directive
/// parsing (issue #879): a challenge page served where `robots.txt` should
/// be aborts discovery with [`CrawlError::WafChallenge`] instead of being
/// silently treated as "no robots.txt". Every other failure mode keeps the
/// legacy `None` ("not found") semantics.
async fn fetch_robots_sitemap(
    base: &Url,
    client: &wreq::Client,
) -> Result<Option<String>, CrawlError> {
    let Ok(robots_url) = base.join("/robots.txt") else {
        tracing::warn!(base_url = %base, "invalid base URL for robots.txt");
        return Ok(None);
    };
    tracing::info!("Checking robots.txt: {}", robots_url);

    let Ok(response) = client.get(robots_url.as_str()).send().await else {
        return Ok(None);
    };
    let status = response.status();
    tracing::info!("robots.txt status: {}", status);
    if !status.is_success() {
        return Ok(None);
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let Ok(content) = response.text().await else {
        return Ok(None);
    };

    // Issue #879: inspect the body before directive parsing — Challenge-tier
    // markers block at any status; Fingerprint-tier evidence never blocks at
    // 2xx (REQ-WAF-09), so benign vendor mentions keep flowing through.
    let ctx = InspectionContext {
        status: Some(status.as_u16()),
        content_type: (!content_type.is_empty()).then_some(content_type),
        headers: std::collections::HashMap::new(),
        ignore_waf: false,
    };
    let verdict = waf_inspector().inspect(&content, &ctx);
    if verdict.is_blocked {
        tracing::warn!(
        url = %robots_url,
        status = status.as_u16(),
        evidences = verdict.evidences.len(),
        "WAF/CAPTCHA challenge detected in robots.txt body; aborting sitemap discovery"
                );
        return Err(CrawlError::WafChallenge {
            provider: verdict.evidence_chain(),
            kind: WafDetectionKind::BodySignature,
            url: robots_url.to_string(),
        });
    }

    log_robots_content(&content);
    Ok(extract_robots_sitemap_directive(&content, base))
}

/// Log the robots.txt body (capped to the first 500 chars for instrumentation).
fn log_robots_content(content: &str) {
    tracing::info!(
        "robots.txt content (first 500 chars):\n{}",
        &content[..content.len().min(500)]
    );
}

/// Parse the first `Sitemap:` directive from a `robots.txt` body and resolve it
/// against `base` (absolute `http(s)` URLs are parsed directly).
fn extract_robots_sitemap_directive(content: &str, base: &Url) -> Option<String> {
    for line in content.lines() {
        if !line.to_lowercase().starts_with("sitemap:") {
            continue;
        }
        let Some(directive) = line
            .strip_prefix("Sitemap:")
            .or_else(|| line.strip_prefix("sitemap:"))
        else {
            continue;
        };
        let sitemap = directive.trim();

        let resolved = if sitemap.starts_with("http://") || sitemap.starts_with("https://") {
            Url::parse(sitemap).ok()
        } else {
            base.join(sitemap).ok()
        };

        match resolved {
            Some(url) => {
                tracing::debug!("Found sitemap in robots.txt: {}", url);
                return Some(url.to_string());
            },
            None => tracing::warn!(directive = %sitemap, "invalid sitemap URL in robots.txt"),
        }
    }
    None
}

/// HEAD-probe each candidate path and return the first that responds 2xx.
async fn probe_sitemap_paths(base: &Url, client: &wreq::Client, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Some(found) = probe_single_path(base, client, path).await {
            return Some(found);
        }
    }
    None
}

/// HEAD-probe a single candidate path; returns its URL when the server answers
/// 2xx, otherwise `None`.
async fn probe_single_path(base: &Url, client: &wreq::Client, path: &str) -> Option<String> {
    let Ok(sitemap_url) = base.join(path) else {
        tracing::warn!(path = %path, "invalid sitemap candidate path");
        return None;
    };
    probe_head_success(client, sitemap_url.as_str()).await
}

/// HEAD-probe a resolved sitemap URL, returning it when the server answers 2xx.
///
/// Servers that reject HEAD (405/501) or answer any other non-2xx to HEAD while
/// still serving the resource over GET get ONE GET retry: a HEAD-only failure
/// must not silently kill sitemap discovery (stabilization-sitemap-regression,
/// scenario 9).
#[tracing::instrument(skip(client), fields(url = %sitemap_str))]
async fn probe_head_success(client: &wreq::Client, sitemap_str: &str) -> Option<String> {
    tracing::info!("Trying fallback sitemap: {}", sitemap_str);

    let response = client.head(sitemap_str).send().await.ok()?;
    tracing::info!("  Status: {}", response.status());
    if response.status().is_success() {
        tracing::debug!("Found sitemap at fallback location: {}", sitemap_str);
        return Some(sitemap_str.to_string());
    }

    // HEAD was rejected — retry once with GET on the same client.
    tracing::info!(
        url = %sitemap_str,
        status = %response.status(),
        "HEAD rejected, retrying with GET"
    );
    let get_response = client.get(sitemap_str).send().await.ok()?;
    tracing::info!("  GET status: {}", get_response.status());
    if get_response.status().is_success() {
        tracing::debug!("Found sitemap via GET fallback: {}", sitemap_str);
        return Some(sitemap_str.to_string());
    }
    None
}

/// Build nested sub-path sitemap candidates (e.g. `/docs/sitemap.xml`) and probe
/// them, honoring the same 3-level depth cap as `crawl_with_subpath_sitemaps`.
async fn probe_subpath_sitemaps(base: &Url, client: &wreq::Client) -> Option<String> {
    let segments: Vec<_> = base.path().split('/').filter(|s| !s.is_empty()).collect();
    let mut candidates = Vec::with_capacity(segments.len().min(3) * 2);
    for i in 1..=segments.len().min(3) {
        let sub_path = segments[..i].join("/");
        for name in ["sitemap.xml", "sitemap_index.xml"] {
            candidates.push(format!("/{sub_path}/{name}"));
        }
    }
    let refs: Vec<&str> = candidates.iter().map(String::as_str).collect();
    probe_sitemap_paths(base, client, &refs).await
}
