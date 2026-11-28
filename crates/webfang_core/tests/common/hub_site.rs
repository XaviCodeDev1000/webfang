//! The hub-and-leaves site the resume/checkpoint binary tests crawl.
//!
//! One seed page linking [`LEAVES`] leaf pages plus `robots.txt`, which is the
//! smallest shape that exercises a real crawl loop: the seed is exported, every
//! leaf is reachable exactly one hop away, and a resumed run has something to
//! skip and something to re-drive.
//!
//! Two binary tests need exactly this shape (`checkpoint_resume_crash_test` for
//! the E13 page-loss probe and `resume_message_test` for the resume notice), so
//! it lives here once instead of twice: the duplication ratchet refuses a second
//! copy, and the two tests must crawl the SAME site for their numbers to be
//! comparable.
//!
//! Every body clears the minimum-content guard and carries a distinct title, so
//! an exported line is attributable to exactly one page.

use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Leaf pages the seed links to; the seed itself makes `LEAVES + 1` pages.
pub const LEAVES: usize = 6;

/// Per-page body with enough text to clear the minimum-content guard and a
/// distinct title so every exported line is unique.
pub fn article(title: &str) -> String {
    format!(
        "<html><body><article><h1>{title}</h1>\
         <p>{title} carries enough substantive text to clear the minimum content guard comfortably.</p>\
         </article></body></html>"
    )
}

/// The hub body: a title, enough text to clear the content guard, and the
/// links the caller supplies. Shared so a second hub-shaped site cannot drift
/// into its own copy of the same nine lines.
#[must_use]
pub fn hub_body(links: &str) -> String {
    format!(
        "<html><body><article><h1>Hub</h1>\
         <p>The hub page carries enough substantive text to clear the minimum content guard.</p>\
         {links}</article></body></html>"
    )
}

/// Mount the hub at `/` with `links` as its body.
pub async fn mount_hub(server: &MockServer, links: &str) {
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_string(hub_body(links)))
        .mount(server)
        .await;
}

/// Mount an allow-everything `robots.txt`.
pub async fn mount_robots(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("User-agent: *\nAllow: /"))
        .mount(server)
        .await;
}

/// Mount `/` (hub linking every leaf), `/page-{i}` and `robots.txt`.
pub async fn mount_hub_site(server: &MockServer) {
    let links: String = (0..LEAVES)
        .map(|i| format!(r#"<a href="/page-{i}">Page {i}</a>"#))
        .collect();
    mount_hub(server, &links).await;
    for i in 0..LEAVES {
        Mock::given(method("GET"))
            .and(path(format!("/page-{i}")))
            .respond_with(ResponseTemplate::new(200).set_body_string(article(&format!("Page {i}"))))
            .mount(server)
            .await;
    }
    mount_robots(server).await;
}
