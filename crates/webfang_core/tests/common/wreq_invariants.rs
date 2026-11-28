//! Hermetic `wreq` harness: a counting resolver over an observing loopback server.
//!
//! Built for the `wreq 0.16.0` client invariants (`wreq_invariants_test`): no
//! name is ever looked up, no packet leaves the host, and ordering is driven by
//! awaiting the server's journal instead of sleeping.

// Each integration test binary includes this file with `#[path]`, so a binary
// that exercises a subset of the harness would otherwise fail `-D dead-code` on
// the helpers it does not happen to call (`cookie_jar_scope_test` uses the
// resolver path, `cookie_jar_session_test` only the loopback listener). Same
// reason `tests/common/mod.rs` carries the allow.
#![allow(dead_code)]
//!
//! The resolver implements `wreq::dns::Resolve`, whose contract assumes the
//! future is ALWAYS ready and takes `&self` — hence an atomic counter and a
//! `ready` future, never a lock held across an `.await`.

use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use wreq::dns::{Addrs, Name, Resolve, Resolving};

/// A name no resolver ever sees: every client in this harness answers DNS from the test.
pub const FAKE_HOST: &str = "wreq-invariants.invalid";

/// A client whose DNS comes from the test, plus the handle to its call count.
///
/// `configure` applies the per-test builder options (cookie jar, retry
/// policy) after the resolver is installed, so every case shares the same
/// hermetic dial path.
pub fn counting_client(
    addrs: Vec<SocketAddr>,
    configure: impl FnOnce(wreq::ClientBuilder) -> wreq::ClientBuilder,
) -> (wreq::Client, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let resolver = CountingResolver {
        calls: Arc::clone(&calls),
        addrs,
    };
    let builder = configure(
        wreq::Client::builder()
            // No system proxy: the dial must be the one this test observes.
            .no_proxy()
            .dns_resolver(resolver),
    );
    (builder.build().expect("the test client builds"), calls)
}

/// Answers every name with the test's own addresses, counting the calls.
///
/// `Resolve` is polled only when it is already ready and takes `&self`, so the
/// counter is an atomic and the future is a `ready` — no lock ever spans an
/// `.await`.
struct CountingResolver {
    calls: Arc<AtomicUsize>,
    addrs: Vec<SocketAddr>,
}

impl Resolve for CountingResolver {
    fn resolve(&self, _name: Name) -> Resolving {
        let seen = self.calls.fetch_add(1, Ordering::SeqCst);
        let addr = self
            .addrs
            .get(seen)
            .or_else(|| self.addrs.last())
            .copied()
            .expect("the resolver is built with at least one address");
        Box::pin(std::future::ready(Ok(
            Box::new(std::iter::once(addr)) as Addrs
        )))
    }
}

/// One observed request: who connected and what it asked for.
#[derive(Clone, Debug)]
pub struct ObservedRequest {
    /// Peer address the request arrived from.
    pub peer: SocketAddr,
    /// Request method as the server read it.
    pub method: String,
    /// Origin-form path, query stripped.
    pub path: String,
    /// Request headers, names lowercased, values trimmed.
    pub headers: Vec<(String, String)>,
}

impl ObservedRequest {
    /// Value of `name` (case-insensitive), first occurrence.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Journal behind a [`TestServer`].
#[derive(Default)]
struct ServerState {
    requests: Mutex<Vec<ObservedRequest>>,
    connections: Mutex<Vec<SocketAddr>>,
    flaky_hits: AtomicUsize,
    flaky_cookie_hits: AtomicUsize,
}

/// An ephemeral loopback HTTP/1.1 listener that journals every arrival.
///
/// It serves the routes the invariants and the cookie-scope tests need and keeps
/// the connection open, so keep-alive reuse is decided by the CLIENT under test:
///
/// - `/set-cookie` → `200` with `Set-Cookie: probe=1; Path=/`
/// - `/flaky` → `503` on the first arrival, `200` afterwards
/// - `/flaky-cookie` → `503` with `Set-Cookie: probe=1; Path=/` on the first
///   arrival, `200` afterwards (retry + per-request jar)
/// - `/redirect-cookie` → `302` to `/landing` with `Set-Cookie: hop=1; Path=/`
/// - anything else → `200 ok`
#[derive(Clone)]
pub struct TestServer {
    addr: SocketAddr,
    state: Arc<ServerState>,
}

impl TestServer {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("the fixture binds an ephemeral loopback port");
        let addr = listener
            .local_addr()
            .expect("the fixture listener has a local address");
        let state = Arc::new(ServerState::default());
        let accepting = Arc::clone(&state);
        tokio::spawn(async move {
            while let Ok((stream, peer)) = listener.accept().await {
                accepting
                    .connections
                    .lock()
                    .expect("the connection journal is never poisoned")
                    .push(peer);
                tokio::spawn(serve_connection(stream, peer, Arc::clone(&accepting)));
            }
        });
        Self { addr, state }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// A URL for this fixture. The port is deliberately absent: `wreq` keeps a
    /// non-zero port resolved from a custom resolver when the URI carries
    /// none, which is exactly the behaviour under test.
    pub fn url(&self, path: &str) -> String {
        format!("http://{FAKE_HOST}{path}")
    }

    pub fn requests(&self) -> Vec<ObservedRequest> {
        self.state
            .requests
            .lock()
            .expect("the arrival journal is never poisoned")
            .clone()
    }

    pub fn paths(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|entry| entry.path)
            .collect()
    }

    pub fn count_path(&self, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|entry| entry.path == path)
            .count()
    }

    pub fn connection_count(&self) -> usize {
        self.state
            .connections
            .lock()
            .expect("the connection journal is never poisoned")
            .len()
    }
}

/// Serve one connection: read request heads, journal them, answer, repeat.
async fn serve_connection(mut stream: TcpStream, peer: SocketAddr, state: Arc<ServerState>) {
    while let Some(head) = read_head(&mut stream).await {
        let Some((method, path, headers)) = parse_head(&head) else {
            return;
        };
        let request = ObservedRequest {
            peer,
            method,
            path,
            headers,
        };
        let response = response_for(&state, request);
        if stream.write_all(&response).await.is_err() {
            return;
        }
    }
}

/// Read one request head (up to, excluding, the blank line); `None` at EOF.
async fn read_head(stream: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        if let Some(end) = buf.windows(4).position(|window| window == b"\r\n\r\n") {
            return Some(buf[..end].to_vec());
        }
        match stream.read(&mut chunk).await {
            Ok(0) => return None,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(_) => return None,
        }
    }
}

/// A parsed request head: method, path, and lowercase headers.
type ParsedHead = (String, String, Vec<(String, String)>);

/// `(method, path, lowercase headers)` from a request head.
fn parse_head(head: &[u8]) -> Option<ParsedHead> {
    let text = std::str::from_utf8(head).ok()?;
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?;
    let path = target.split('?').next()?.to_string();
    let headers = lines
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect();
    Some((method, path, headers))
}

/// Journal the arrival, then build the response bytes for it.
fn response_for(state: &ServerState, request: ObservedRequest) -> Vec<u8> {
    let path = request.path.clone();
    state
        .requests
        .lock()
        .expect("the arrival journal is never poisoned")
        .push(request);
    match path.as_str() {
        "/flaky" => {
            let hit = state.flaky_hits.fetch_add(1, Ordering::SeqCst);
            if hit == 0 {
                response(503, "Service Unavailable", &[], "retry me")
            } else {
                response(200, "OK", &[], "ok")
            }
        },
        "/set-cookie" => response(200, "OK", &["set-cookie: probe=1; Path=/"], "ok"),
        "/flaky-cookie" => {
            let hit = state.flaky_cookie_hits.fetch_add(1, Ordering::SeqCst);
            if hit == 0 {
                response(
                    503,
                    "Service Unavailable",
                    &["set-cookie: probe=1; Path=/"],
                    "retry me",
                )
            } else {
                response(200, "OK", &[], "ok")
            }
        },
        "/redirect-cookie" => response(
            302,
            "Found",
            &["location: /landing", "set-cookie: hop=1; Path=/"],
            "",
        ),
        _ => response(200, "OK", &[], "ok"),
    }
}

/// Minimal HTTP/1.1 response that keeps the connection open.
fn response(status: u16, reason: &str, headers: &[&str], body: &str) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: keep-alive\r\n",
        body.len()
    );
    for header in headers {
        out.push_str(header);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(body);
    out.into_bytes()
}

// --- cookie assertion helpers ------------------------------------------------
//
// Both cookie-scope suites (`cookie_jar_scope_test`, `cookie_jar_session_test`)
// ask the same two questions of the journal — which URL did the test dial, and
// which cookies did that arrival carry — so the readers live here once instead
// of being copied into each suite (the duplication ratchet counts every copy).

/// The hardened production client, wired as the container wires it.
pub fn hardened_client() -> Arc<webfang_core::application::http_client::HttpClient> {
    use webfang_core::application::http_client::{HttpClient, HttpClientConfig};
    Arc::new(HttpClient::new(HttpClientConfig::default()).expect("the hardened client builds"))
}

/// A loopback URL for `path`: the tests dial the fixture directly, so no
/// resolver — the client's or the system's — answers for it.
pub fn target(server: &TestServer, path: &str) -> String {
    format!("http://{}{path}", server.addr())
}

/// The cookie pairs of the `Cookie` header an arrival carried, order-independent.
pub fn cookie_pairs(server: &TestServer, arrival: usize) -> BTreeSet<String> {
    server.requests()[arrival]
        .header("cookie")
        .unwrap_or_default()
        .split("; ")
        .filter(|pair| !pair.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Drive the two-arrival scenario both scope suites need: a call that stores a
/// cookie (`/set-cookie`) and a follow-up call (`/echo`) whose `Cookie` header is
/// what the assertions read.
pub async fn set_cookie_then_echo(
    client: &dyn webfang_core::domain::ports::HttpClientPort,
    server: &TestServer,
) {
    client
        .get(&target(server, "/set-cookie"))
        .await
        .expect("the cookie-setting request succeeds");
    client
        .get(&target(server, "/echo"))
        .await
        .expect("the follow-up request succeeds");
}

/// The pairs of these cookie names, as the cookie assertions read them.
pub fn pairs(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}
