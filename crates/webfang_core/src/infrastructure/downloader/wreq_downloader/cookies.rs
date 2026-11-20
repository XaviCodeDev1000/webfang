//! Cookie extraction/parsing and header/Retry-After adapters.
//!
//! Pure helpers of the fetch guard chain: they read a response, never dial.
//! Extracted verbatim from `wreq_downloader.rs`.

use std::collections::HashMap;

use url::Url;
use wreq::header::HeaderMap;

use super::Cookie;

/// Extract cookies from a wreq response.
pub(super) fn extract_cookies(url: &Url, response: &wreq::Response) -> Vec<Cookie> {
    let mut cookies = Vec::new();

    // Extract cookies from the cookie store via the response cookies
    for cookie in response.cookies() {
        cookies.push(Cookie {
            name: cookie.name().to_string(),
            value: cookie.value().to_string(),
            domain: cookie.domain().unwrap_or("").to_string(),
            path: cookie.path().unwrap_or("/").to_string(),
            http_only: cookie.http_only(),
            secure: cookie.secure(),
        });
    }

    // Also extract Set-Cookie headers for cookies not in the store
    let set_cookie_headers = response.headers().get_all("set-cookie");
    let existing_names: std::collections::HashSet<_> =
        cookies.iter().map(|c| c.name.clone()).collect();

    for header_value in set_cookie_headers {
        if let Ok(value_str) = header_value.to_str() {
            // Parse basic cookie fields from Set-Cookie header
            if let Some(cookie) = parse_set_cookie(value_str, url) {
                if !existing_names.contains(&cookie.name) {
                    cookies.push(cookie);
                }
            }
        }
    }

    cookies
}

/// Parse a Set-Cookie header value into a Cookie struct.
pub(super) fn parse_set_cookie(header: &str, url: &Url) -> Option<Cookie> {
    let parts: Vec<&str> = header.split(';').collect();
    if parts.is_empty() {
        return None;
    }

    let name_value = parts[0].trim();
    let pos = name_value.find('=')?;
    let (name, value) = (
        name_value[..pos].trim().to_string(),
        name_value[pos + 1..].trim().to_string(),
    );

    if name.is_empty() {
        return None;
    }

    let mut domain = url.host_str().unwrap_or("").to_string();
    let mut path = "/".to_string();
    let mut http_only = false;
    let mut secure = false;

    for part in &parts[1..] {
        let part = part.trim().to_lowercase();
        if let Some(val) = part.strip_prefix("domain=") {
            domain = val.trim().to_string();
        } else if let Some(val) = part.strip_prefix("path=") {
            path = val.trim().to_string();
        } else if part == "httponly" {
            http_only = true;
        } else if part == "secure" {
            secure = true;
        }
    }

    Some(Cookie {
        name,
        value,
        domain,
        path,
        http_only,
        secure,
    })
}

/// Parse the `Retry-After` header (integer seconds) into milliseconds.
///
/// Returns the delay in ms, defaulting to 1000ms if the header is absent or unparseable.
pub(super) fn parse_retry_after_ms(response: &wreq::Response, max_ms: u64) -> u64 {
    let raw_ms = response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(1)
        .saturating_mul(1000);
    raw_ms.min(max_ms)
}

/// Collect a `wreq` header map into the lowercased, single-valued form the WAF
/// inspection boundary expects (REQ-WAF-01).
///
/// This adapter — and not the inspection logic — is what each HTTP stack keeps local,
/// because `domain` must not learn `wreq` types. Duplicate header names collapse with
/// last-value-wins, which is sound here: every WAF control header is single-valued.
pub(super) fn lowercase_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_lowercase(), v.to_string()))
        })
        .collect()
}
