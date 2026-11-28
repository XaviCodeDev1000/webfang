#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! `session` cookie jar scope through the REAL MCP composition (T2a-2).
//!
//! These are the named tests of the slice that only the transport can ask: the
//! session identity comes from the `Mcp-Session-Id` header of a live Streamable
//! HTTP request, so nothing below the router can fake it. Each test drives two
//! real MCP sessions against one server and reads the WIREMOCK journal — what
//! the site actually received — never the server's opinion of itself.
//!
//! | Test | Spec question |
//! |------|---------------|
//! | `scrape_url_does_not_carry_a_cookie_across_sessions` | (a) A→B negative for `scrape_url` |
//! | `scrape_url_offers_the_cookie_on_the_next_call_of_the_same_session` | (b) call 1 → call 2 inside one session |
//! | `scrape_batch_does_not_carry_a_cookie_across_sessions` | (a) A→B negative for `scrape_batch` |
//!
//! The `shared` default is what the two sessions would otherwise share, so the
//! negative cases are the ones that prove the scope is wired at all.

mod common;

use common::{
    anonymous_server_options, call_tool, init_session, init_ssrf_disabled, serve_on_random_port,
};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use webfang_core::application::container::Container;
use webfang_core::config::Config;
use webfang_core::domain::JarScope;
use webfang_mcp::mcp_server::server::build_mcp_router;
use webfang_mcp::mcp_server::state::McpState;

/// A minimal page Readability can extract, so a scrape completes instead of
/// failing before the request is even sent.
const PAGE: &str = "<html><head><title>Fixture</title></head><body><article>\
<p>Enough content for the extraction stage to produce a result.</p></article></body></html>";

/// The real MCP router serving from a `session`-scoped container.
async fn start_session_scoped_server() -> (String, tokio::task::JoinHandle<()>) {
    init_ssrf_disabled();
    let config = Config::default();
    let container = Container::new(config.crawler, config.scraper)
        .await
        .expect("the container builds")
        .with_cookie_jar_scope(JarScope::Session, None);
    let app = build_mcp_router(McpState::new(container), &anonymous_server_options());
    serve_on_random_port(app).await
}

/// A site that stores a cookie on `/login` and echoes nothing on `/echo`.
///
/// Neither route inspects the `Cookie` header: the assertion reads the journal,
/// so the mocks stay indifferent to what they are proving.
async fn cookie_site() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("set-cookie", "probe=1; Path=/")
                .set_body_string(PAGE),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/echo"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PAGE))
        .mount(&server)
        .await;
    server
}

/// The `Cookie` headers the site received on `route`, in arrival order.
async fn cookies_sent_to(site: &MockServer, route: &str) -> Vec<String> {
    let requests = site
        .received_requests()
        .await
        .expect("the fixture journals every arrival");
    requests
        .iter()
        .filter(|request| request.url.path() == route)
        .map(|request| {
            request
                .headers
                .get("cookie")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

/// Run `tool` in session A against `/login`, then in a FRESH session B against
/// `/echo`, and return the `Cookie` headers `/echo` received.
///
/// `arguments` builds the tool's arguments from the URL it must scrape, so the
/// two tools that have a negative case (`scrape_url`, `scrape_batch`) share one
/// scenario instead of one copy of it each.
async fn echo_cookies_of_a_fresh_session_for(
    tool: &str,
    arguments: fn(String) -> serde_json::Value,
) -> Vec<String> {
    let (base_url, _server) = start_session_scoped_server().await;
    let site = cookie_site().await;
    let client = wreq::Client::new();

    let session_a = init_session(&client, &base_url).await;
    call_tool(
        &client,
        &base_url,
        &session_a,
        tool,
        arguments(format!("{}/login", site.uri())),
    )
    .await;
    let session_b = init_session(&client, &base_url).await;
    call_tool(
        &client,
        &base_url,
        &session_b,
        tool,
        arguments(format!("{}/echo", site.uri())),
    )
    .await;

    cookies_sent_to(&site, "/echo").await
}

/// (a) `scrape_url`: a cookie session A stored is never offered on session B's
/// request.
#[tokio::test]
async fn scrape_url_does_not_carry_a_cookie_across_sessions() {
    let echoed =
        echo_cookies_of_a_fresh_session_for("scrape_url", |url| json!({ "url": url })).await;

    assert_eq!(
        echoed,
        vec![String::new()],
        "session B must not be offered the cookie session A stored"
    );
}

/// (b) `scrape_url`: the same session's next call IS offered the cookie, which is
/// what the scope exists for (a load-balancing or consent cookie reused between
/// calls instead of being re-acquired).
#[tokio::test]
async fn scrape_url_offers_the_cookie_on_the_next_call_of_the_same_session() {
    let (base_url, _server) = start_session_scoped_server().await;
    let site = cookie_site().await;
    let client = wreq::Client::new();

    let session = init_session(&client, &base_url).await;
    call_tool(
        &client,
        &base_url,
        &session,
        "scrape_url",
        json!({ "url": format!("{}/login", site.uri()) }),
    )
    .await;
    call_tool(
        &client,
        &base_url,
        &session,
        "scrape_url",
        json!({ "url": format!("{}/echo", site.uri()) }),
    )
    .await;

    let echoed = cookies_sent_to(&site, "/echo").await;
    assert_eq!(echoed.len(), 1, "exactly one request reached `/echo`");
    assert!(
        echoed[0].contains("probe=1"),
        "the second call of the same session is offered the stored cookie; it \
         carried {:?} instead",
        echoed[0]
    );
}

/// (a) `scrape_batch`: the batch path goes through the same scoped client, so the
/// negative must hold there too.
#[tokio::test]
async fn scrape_batch_does_not_carry_a_cookie_across_sessions() {
    let echoed =
        echo_cookies_of_a_fresh_session_for("scrape_batch", |url| json!({ "urls": [url] })).await;

    assert_eq!(
        echoed,
        vec![String::new()],
        "a batch of session B must not be offered the cookie session A stored"
    );
}
