//! Hub-and-leaves site for the semantic-cleaning degradation tests (T6, E9).
//!
//! The shape that makes a PARTIAL degradation reproducible from the real
//! binary: one seed linking a leaf that cleans fine and a leaf whose readable
//! text is a SINGLE paragraph longer than the run's `--max-chars` budget.
//! [`crate::infrastructure::semantic_cleaner::text::split_into_chunks`] cannot
//! split one paragraph, so that page fails with `ChunkTooLarge` — the one
//! `SemanticError` variant the shipped cleaner can actually produce — while
//! the rest of the run cleans normally.
//!
//! Kept here instead of in the test module for the reason the duplication
//! ratchet exists: the resume-notice tests already own the plain hub site
//! ([`super::hub_site`]), and a second copy of "hub + leaves + robots.txt"
//! inside a new test file is exactly the drift that ratchet refuses. This one
//! reuses [`super::hub_site::article`] for the leaves that must NOT degrade, so
//! the two sites agree on what "a page that clears the content guard" means.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Path of the leaf that cleans normally.
pub const CLEAN_PATH: &str = "/clean";

/// Path of the leaf whose text cannot be chunked under the test budget.
pub const DEGRADED_PATH: &str = "/long";

/// Title of the degraded leaf, so the exported line is attributable.
pub const DEGRADED_TITLE: &str = "Oversized";

/// The `--max-chars` budget the degradation tests run with.
///
/// Larger than [`super::hub_site::article`]'s paragraph (the leaves that must
/// clean) and far smaller than [`oversized_article`]'s single paragraph (the
/// leaf that must not), so the run degrades exactly one of its pages.
pub const MAX_CHARS: &str = "200";

/// A body whose readable text is ONE paragraph of roughly `chars` characters.
///
/// No newline and no block-level sibling: the cleaner reads newlines as
/// paragraph boundaries, so a body that carried one would be splittable and
/// would never reach `ChunkTooLarge`.
#[must_use]
pub fn oversized_article(title: &str, chars: usize) -> String {
    let sentence = "This leaf carries one unbroken paragraph of substantive text that the cleaner cannot split under the test budget. ";
    let mut body = String::new();
    while body.chars().count() < chars {
        body.push_str(sentence);
    }
    format!("<html><body><article><h1>{title}</h1><p>{body}</p></article></body></html>")
}

/// Mount `/` (hub linking both leaves), the two leaves and `robots.txt`.
pub async fn mount_cleaning_site(server: &MockServer) {
    let links = format!(r#"<a href="{CLEAN_PATH}">Clean</a><a href="{DEGRADED_PATH}">Long</a>"#);
    // The hub body and the robots mount are the resume tests' own fixtures:
    // reusing them keeps "a page that clears the content guard" one definition.
    super::hub_site::mount_hub(server, &links).await;
    Mock::given(method("GET"))
        .and(path(CLEAN_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string(super::hub_site::article("Clean")))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(DEGRADED_PATH))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(oversized_article(DEGRADED_TITLE, 600)),
        )
        .mount(server)
        .await;
    super::hub_site::mount_robots(server).await;
}
