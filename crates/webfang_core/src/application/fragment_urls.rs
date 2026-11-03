//! URL absolutization for the selector fragment (#2534, #2554).
//!
//! The selector path serializes matched elements and hands them to consumers as
//! `html`, so every URL in that fragment has to be absolute — Readability used
//! to do this implicitly and no longer runs on that path.
//!
//! `srcset` needs special care: it holds a candidate LIST, and splitting it on
//! commas corrupts URLs that legitimately contain them (image CDNs emit
//! `/w_100,h_100/foto.jpg`, `data:` URIs contain them too). `parse_srcset`
//! follows the HTML parsing algorithm for the attribute instead.

/// Resolve `href`/`src`/`poster`/`srcset` in `fragment` against `base_url`.
///
/// Fragment-only links are left alone: rewriting `#top` into an absolute URL
/// changes what the link means. Values `Url::join` cannot resolve keep their
/// original text rather than losing the link, and a rewriter failure returns
/// the fragment untouched — the fragment is never dropped on the floor.
pub(crate) fn absolutize_urls(fragment: &str, base_url: &url::Url) -> String {
    use lol_html::{element, RewriteStrSettings};

    let settings = RewriteStrSettings {
        element_content_handlers: vec![
            element!("a[href]", |el| {
                resolve_attr(el, "href", base_url);
                Ok(())
            }),
            element!("img[src]", |el| {
                resolve_attr(el, "src", base_url);
                Ok(())
            }),
            element!("img[srcset]", |el| {
                resolve_srcset_attr(el, base_url);
                Ok(())
            }),
            element!("source[src]", |el| {
                resolve_attr(el, "src", base_url);
                Ok(())
            }),
            element!("source[srcset]", |el| {
                resolve_srcset_attr(el, base_url);
                Ok(())
            }),
            element!("video[poster]", |el| {
                resolve_attr(el, "poster", base_url);
                Ok(())
            }),
        ],
        ..RewriteStrSettings::default()
    };

    lol_html::rewrite_str(fragment, settings).unwrap_or_else(|_| fragment.to_owned())
}

/// Rewrite one attribute in place when it resolves against `base_url`.
fn resolve_attr(el: &mut lol_html::html_content::Element<'_, '_>, attr: &str, base_url: &url::Url) {
    let Some(raw) = el.get_attribute(attr) else {
        return;
    };
    if raw.starts_with('#') {
        return;
    }
    if let Ok(resolved) = base_url.join(&raw) {
        let _ = el.set_attribute(attr, resolved.as_str());
    }
}

/// Rewrite a `srcset` attribute in place, leaving it untouched when no
/// candidate changes.
fn resolve_srcset_attr(el: &mut lol_html::html_content::Element<'_, '_>, base_url: &url::Url) {
    let Some(raw) = el.get_attribute("srcset") else {
        return;
    };
    let rewritten = rewrite_srcset(&raw, base_url);
    if rewritten != raw {
        let _ = el.set_attribute("srcset", &rewritten);
    }
}

/// Split a `srcset` value into `(url, descriptor)` candidates.
///
/// The descriptor is empty when the candidate has none. Per the HTML
/// algorithm: skip whitespace and leading commas, take the URL as the run up to
/// the next whitespace, strip trailing commas (that candidate carries no
/// descriptor), otherwise read the descriptor up to the first comma that is not
/// inside parentheses.
fn parse_srcset(value: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = value.chars().collect();
    let mut candidates = Vec::new();
    let mut i = 0;

    loop {
        while i < chars.len() && (chars[i].is_whitespace() || chars[i] == ',') {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        let url_start = i;
        while i < chars.len() && !chars[i].is_whitespace() {
            i += 1;
        }
        let mut url: String = chars[url_start..i].iter().collect();

        // A URL ending in commas is a complete candidate with no descriptor;
        // the commas are separators, never part of the URL.
        let mut descriptor = String::new();
        if url.ends_with(',') {
            while url.ends_with(',') {
                url.pop();
            }
        } else if i < chars.len() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let desc_start = i;
            let mut depth = 0usize;
            while i < chars.len() {
                match chars[i] {
                    '(' => depth += 1,
                    ')' => depth = depth.saturating_sub(1),
                    ',' if depth == 0 => break,
                    _ => {},
                }
                i += 1;
            }
            descriptor = chars[desc_start..i].iter().collect::<String>();
            descriptor = descriptor.trim().to_owned();
            // The terminating comma is left for the next iteration's skip.
        }

        if !url.is_empty() {
            candidates.push((url, descriptor));
        }
    }

    candidates
}

/// Resolve every candidate in `value` against `base_url`.
///
/// Descriptors are kept verbatim, candidates are reassembled with `", "`, and
/// a URL that is fragment-only or cannot be resolved keeps its original text —
/// the same contract `resolve_attr` follows for single attributes.
fn rewrite_srcset(value: &str, base_url: &url::Url) -> String {
    let rewritten: Vec<String> = parse_srcset(value)
        .into_iter()
        .map(|(url, descriptor)| {
            let absolute = if url.starts_with('#') {
                url
            } else {
                match base_url.join(&url) {
                    Ok(resolved) => resolved.to_string(),
                    Err(_) => url,
                }
            };
            if descriptor.is_empty() {
                absolute
            } else {
                format!("{absolute} {descriptor}")
            }
        })
        .collect();
    rewritten.join(", ")
}

#[cfg(test)]
mod tests {
    use super::{parse_srcset, rewrite_srcset};

    fn base() -> url::Url {
        url::Url::parse("https://e.com/dir/page").expect("valid base URL")
    }

    /// #2554 acceptance table: one case per shape the HTML algorithm has to
    /// survive. A naive comma split fails cases 3, 4 and 6.
    #[test]
    fn issue_2554_srcset_candidates_are_absolutized() {
        let cases: [(&str, &str); 7] = [
            ("/a.jpg", "https://e.com/a.jpg"),
            (
                "/a.jpg 1x, /b.jpg 2x",
                "https://e.com/a.jpg 1x, https://e.com/b.jpg 2x",
            ),
            (
                "/w_100,h_100/foto.jpg 1x, /w_200,h_200/foto.jpg 2x",
                "https://e.com/w_100,h_100/foto.jpg 1x, https://e.com/w_200,h_200/foto.jpg 2x",
            ),
            (
                "/a.jpg, /b.jpg 2x",
                "https://e.com/a.jpg, https://e.com/b.jpg 2x",
            ),
            (
                "/a.jpg 480w, /b.jpg 960w",
                "https://e.com/a.jpg 480w, https://e.com/b.jpg 960w",
            ),
            (
                "data:image/png;base64,AAA 1x",
                "data:image/png;base64,AAA 1x",
            ),
            ("https://cdn/x.jpg 1x", "https://cdn/x.jpg 1x"),
        ];

        for (input, expected) in cases {
            assert_eq!(rewrite_srcset(input, &base()), expected, "case {input:?}");
        }
    }

    /// #2554 round trip: parsing the rewritten value yields the same
    /// candidates, so the rewrite is stable in shape.
    #[test]
    fn issue_2554_rewritten_srcset_round_trips() {
        let input = "/w_100,h_100/foto.jpg 1x, /a.jpg, data:image/png;base64,AA,BB 2x";
        let once = rewrite_srcset(input, &base());
        let twice = rewrite_srcset(&once, &base());
        assert_eq!(once, twice, "rewrite must be stable");
        assert_eq!(
            parse_srcset(&once),
            parse_srcset(&twice),
            "candidates must survive a round trip"
        );
    }

    /// #2554: a fragment-only candidate is left alone, exactly like
    /// `resolve_attr` does for `href`.
    #[test]
    fn issue_2554_fragment_only_candidate_is_untouched() {
        assert_eq!(
            rewrite_srcset("#sprite 1x", &base()),
            "#sprite 1x",
            "a fragment is not a URL to absolutize"
        );
    }
}
