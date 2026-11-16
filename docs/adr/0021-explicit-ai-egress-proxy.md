# ADR 0021: Explicit Per-Provider Forward Proxy for AI Egress — Type-Separated SSRF Policies and Zero Ambient Env Reads

- **Status:** Proposed — draft for owner review. Authorizes no code until approved.
- **Date:** 2026-11-16
- **Deciders:** Project owner (`Xavi`), architecture review
- **Related:** `ADR-0003` (remote OpenAI-compatible inference), `ADR-0004` (local ONNX removal, B1 `#1462` `allow_loopback`, B2 `#1947` `NetworkPolicy` + FIN-017), `ADR-0010` / `ADR-0012` (intra-crate dependency direction), `ADR-0018` (crawl egress strategy without third-party proxies), `ADR-0020` (shared LLM transport primitives)
- **Docs:** `docs/src/data-flow.md` (§ "Proxy posture"), `docs/src/installation.md` (§ "Behind a corporate proxy?"), `docs/ssrf-layers.md`, `AGENTS.md` § "Fetch guard chain"
- **Issues:** follow-up from `#2500` (closed by `#2547`)
- **Supersedes:** — (narrows `#2547`'s blanket non-feature posture for AI provider egress only; preserves `.no_proxy()` against the OS environment everywhere)

---

## 1. Context

### 1.1 The operator gap left by `#2547`

Issue `#2500` (closed by `#2547`) pinned `.no_proxy()` on every client built through
`DefaultSsrfGuard::secure_client*` (`crates/webfang_core/src/infrastructure/ssrf.rs`) so
`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and `NO_PROXY` are ignored across the entire
product. Since `ADR-0004` step 6 removed local ONNX inference, every `--clean-ai`,
vault-search, and `--extract-with-llm` run requires network egress to an OpenAI-compatible
endpoint.

Two operator profiles cannot use direct dials:

1. **Corporate operator behind a mandatory forward proxy** — direct outbound TCP/443 is
   dropped at the perimeter; internal machines frequently lack external DNS resolution
   (split-horizon DNS, where only the proxy resolves public names).
2. **Egress-governance operator** — routes AI traffic through an auditable forward
   chokepoint without rewriting `base_url` or running an extra loopback sidecar process per
   host.

### 1.2 Verified transport mechanics (`wreq` 6.0.0-rc.29)

1. **`wreq` carries a single DNS resolver per `Client`:** `Connector::builder(proxies, resolver)`
   uses the installed `ValidatingResolver` to resolve **the socket peer**. Under a proxy,
   `connect_via_proxy` rewrites the dial target to the proxy URI: the resolver checks the
   **proxy's** hostname, never the destination's.
2. **`wreq` bypasses custom resolvers for IP literals:** `HttpConnector::call` parses
   literal IP hosts directly (`dns::SocketAddrs::try_parse`) and never calls
   `ValidatingResolver`. Because corporate proxies are commonly configured by RFC1918 IP
   literal (e.g. `http://10.20.30.40:3128`), a resolver-only guard would let a forbidden
   literal proxy dial straight through unless an entry gate checks it first.
3. **Why client-side pre-resolution of the destination is an anti-pattern:**
   Resolving `base_url` locally before issuing HTTP `CONNECT` fails immediately on
   split-horizon corporate networks (where local `getaddrinfo` returns `NXDOMAIN` for
   external hosts) and introduces a classic DNS-rebinding TOCTOU window (client resolves a
   public IP; proxy resolves an internal IP milliseconds later). Under HTTP `CONNECT` (for
   `https://` targets, preserving BoringSSL `Chrome145` TLS end-to-end via
   `TunnelConnector`) and absolute-form forwarding (for `http://` targets), destination
   name resolution belongs to the proxy by protocol construction.
4. **Why sharing one `NetworkPolicy` between destination and proxy is a vulnerability:**
   A typical corporate setup targets a **public** AI endpoint (`https://api.openai.com/v1`)
   through a **private** RFC1918 proxy (`http://10.20.30.40:3128`). If `10.20.0.0/16` were
   added to the provider's destination `network_policy`, `redirect_policy_with_policy`
   (stage 3b) would permit a malicious or compromised upstream `302` redirect to
   `http://10.20.30.99/` through the proxy. Proxy dial permissions and destination/redirect
   permissions are distinct trust domains and must be separated in the type system.
5. **Current asymmetry in `infrastructure/llm/`:** `RemoteEmbeddingAdapter` builds a
   per-provider policy-aware client (`embedding_http_client`), whereas
   `OpenAiCompatibleProvider` and `webfang_cli` still call `build_default_http_client()`
   with a plain `secure_client(builder)`. `ADR-0020` Slice 2 already decided to unify both
   adapters behind one `infrastructure/llm/http_client.rs` builder.

---

## 2. Decision

We resolve the four design questions in dependency order.

### Decision 1 — Declaration surface: config-only `[[proxies]]` catalog + per-provider reference or inline table; zero env reads, zero CLI URL flags, zero inline URL credentials

1. **Ambient proxy environment variables remain unconditionally ignored.** Every
   `wreq::ClientBuilder` continues to call `.no_proxy()` first to strip `wreq`'s default
   `ProxyMatcher::system()`. Explicit proxy configuration is attached **after**
   `.no_proxy()`, so `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` never
   participate in routing on any platform.
2. **No CLI `--proxy` flag.** Per the `allow_loopback` precedent (`domain/providers.rs`),
   CLI flags leak into OS process tables (`ps`) and shell history, and an RFC1918 proxy
   requires structured `network_policy` declarations that live in `config.toml` anyway.
3. **Reusable `[[proxies]]` catalog + inline shorthand in `config.toml`:**
   Because an operator behind a corporate proxy typically configures both `embedding` and
   `completion` providers, `ConfigDefaults` gains a top-level `[[proxies]]` list (`id` +
   proxy definition), and `ProviderConfig` gains an optional `proxy` field (`ProxySpec`)
   that accepts either a string `id` referencing `[[proxies]]` or an inline table:

   ```toml
   [[proxies]]
   id = "corp-egress"
   url = "http://proxy.corp.internal:3128"
   allow_loopback = false
   network_policy = { mode = "allowlist", allow_cidrs = ["10.20.0.0/16"], allow_hosts = ["proxy.corp.internal"] }
   # Optional Proxy-Authorization Basic credential (never inline in `url` or TOML):
   # auth = { user = "svc-webfang", password = { source = "encrypted_file", path = "/home/op/.config/webfang/proxy.age" } }

   [[providers]]
   id = "openai-corp"
   display_name = "OpenAI via Corp Proxy"
   kind = "open_ai_compatible"
   base_url = "https://api.openai.com/v1"
   capabilities = ["embedding", "completion"]
   model = "text-embedding-3-small"
   auth = { source = "encrypted_file", path = "/home/op/.config/webfang/openai.age" }
   proxy = "corp-egress"
   ```

   Explicit `null` on `proxy` is a deserialization error (fail-loud, matching
   `deserialize_dim_no_null`). Duplicate `[[proxies]]` IDs or an unknown `proxy = "id"`
   fail at startup with `CliExit::ConfigError` (exit 78) and a Spanish error message.
4. **Zero inline secrets in `url`:** A `proxy.url` carrying userinfo
   (`!url.username().is_empty() || url.password().is_some()`) is **rejected at validation**
   with exit 78 pointing to `proxy.auth`. `url::Url` stores credentials in plain,
   non-zeroized heap strings that leak on `Display`; proxy authentication (`Basic`) instead
   composes a plain `user: String` with `password: AuthSource` (resolving once at startup
   into a zeroized `ApiKey` / `SecretString`, passed to `wreq::Proxy::basic_auth`).

---

### Decision 2 — Guard semantics under a proxy: type-separated `EndpointPolicy` vs `ValidatedProxy`, `http`/`https` only in v1, explicit destination-DNS delegation

1. **Type-driven separation of destination and proxy policies (no boolean-parameter
   sprawl):**
   Instead of adding positional `bool` and `&NetworkPolicy` tuples to `SsrfGuard`,
   `domain` defines two distinct, validated security descriptors:
   - `EndpointPolicy` — `{ allow_loopback: bool, network_policy: NetworkPolicy }` governing
     the **destination** (`provider.base_url` entry check and stage 3b literal-IP redirect
     stops).
   - `ValidatedProxy` — an opaque domain value object (private fields) constructible
     **only** via `ValidatedProxy::try_new(config: &ProxyConfig) -> Result<Self, ProxyConfigError>`.
     Its constructor enforces stage 1 **on the proxy URL itself**:
     - Allowed schemes: **`http` and `https` only** in v1.
     - No userinfo, non-empty host, no path/query/fragment ambiguity beyond origin.
     - Policy-aware literal-IP gate: `is_forbidden_literal_host_with_policy(proxy_host, proxy.allow_loopback, &proxy.network_policy)`.
       FIN-017 (`NEVER_ALLOWABLE`: `169.254.0.0/16`, `0.0.0.0/8`, multicast, reserved,
       Teredo, non-canonical NAT64, 6to4) remains unconditionally denied even for a proxy.
   - `ClientSecurityProfile<'a>` pairs `destination: EndpointPolicy` with
     `via_proxy: Option<&'a ValidatedProxy>`. `SsrfGuard` gains one additive method,
     `secure_client_with_profile(&self, builder, profile: &ClientSecurityProfile<'_>)`
     (keeping the three existing methods intact for callers that do not use profiles):
     - **Redirect guard (stage 3b):** wired to `profile.destination`
       (`redirect_policy_with_policy(dest.allow_loopback, dest.network_policy)`). Allowing
       an RFC1918 CIDR for the proxy **never** opens literal redirects into that CIDR.
     - **Proxy matcher:** calls `.no_proxy()` unconditionally, then—if `via_proxy` is
       `Some(proxy)`—installs `wreq::Proxy::all(...)` (plus resolved `basic_auth` when
       configured).
     - **Connect-time DNS guard (stage 3c):** when `via_proxy` is `None`,
       `ValidatingResolver` carries `profile.destination`; when `via_proxy` is
       `Some(proxy)`, `ValidatingResolver` carries `proxy.endpoint_policy()`, enforcing the
       `allow_cidrs` **AND** `allow_hosts` anti-rebinding check on every proxy socket dial.
2. **SOCKS (`socks5` / `socks5h`) rejected explicitly in v1:**
   `Cargo.toml` compiles `wreq` without the `socks` feature. More importantly:
   - `socks5://` (local DNS) would ask the client's single `ValidatingResolver` to resolve
     *both* the proxy host and the destination host under a single policy, conflating the
     two trust domains.
   - `socks5h://` (remote DNS) delegates hostname resolution identically to HTTP `CONNECT`,
     but adds dependency surface (`Cargo.lock`, SBOM, `THIRD_PARTY_NOTICES.txt`) without a
     verified operator need that HTTP `CONNECT` does not already cover.
   Both `socks5` and `socks5h` (and any other non-`http(s)` scheme) are rejected at
   `ValidatedProxy::try_new` with a specific Spanish diagnostic explaining that only
   `http://` and `https://` forward proxies (HTTP `CONNECT` tunneling for HTTPS targets)
   are supported.
3. **Explicit responsibility boundary under a configured proxy:**

   | Protection / Stage | Without proxy | With `proxy` configured on AI provider |
   | :--- | :--- | :--- |
   | Stage 1 on `base_url` (scheme + literal-IP gate) | Client-enforced (`destination` policy) | **Client-enforced** (`destination` policy; `http://127.0.0.1` or `http://169.254.169.254` in `base_url` still fails pre-socket) |
   | Stage 1 + Stage 3c on **proxy** host (literal + DNS rebinding AND) | n/a | **Client-enforced** (`proxy` policy + FIN-017 never-allowable) |
   | Pacing / budgets & `--offline` gate (exit 78) | Client-enforced | **Client-enforced** (`--offline` still rejects before any proxy dial) |
   | Stage 3a Timeouts (10 s connect / 60 s request) | Client-enforced | **Client-enforced** |
   | Stage 3b Redirect policy (10-hop cap + literal-IP stop) | Client-enforced (`destination` policy) | **Client-enforced** (`destination` policy, **not** proxy's CIDR) |
   | Stage 3c Destination hostname → IP validation | Client-enforced (`ValidatingResolver`) | **Delegated to the proxy** (`CONNECT host:port` / absolute-URI; proxy resolves destination and redirect hostnames) |
   | Stage 4 Retry classification & `Retry-After` ceiling | Client-enforced | **Client-enforced** (407 Proxy Auth Required treated as terminal config/auth failure, never retried) |
   | Stage 5 Bounded body read (`read_body_capped`, 16 MiB) | Client-enforced | **Client-enforced** |
   | TLS to `https://` `base_url` | End-to-end (`Chrome145`) | **End-to-end** inside `CONNECT` tunnel (`TunnelConnector`) |

---

### Decision 3 — Scope: AI egress only (both OpenAI-compatible adapters via `ADR-0020` shared builder); crawl/scrape stays direct-only

1. **In scope — both OpenAI-compatible AI adapters:**
   - `RemoteEmbeddingAdapter` (`POST {base_url}/embeddings` — `--clean-ai`, vault search,
     Tier 2 opt-in, `webfang doctor`, `webfang connect` validation, MCP `semantic_cleaner`).
   - `OpenAiCompatibleProvider` (`POST {base_url}/chat/completions` — `--extract-with-llm`).
   - Both adapters construct their `wreq::Client` per provider through the shared
     `infrastructure/llm/http_client.rs` primitive (`ADR-0020` Slice 2), eliminating the
     process-wide plain-guard client in `webfang_cli` / `provider.rs` so completion
     providers honour `allow_loopback`, `network_policy`, and `proxy` symmetrically with
     embedding providers (and `llm_extraction::ssrf_gate` becomes policy-aware to match
     `remote_embedding::entry_gate`).
   - HTTP `407 Proxy Authentication Required` is classified explicitly (mirroring `401`
     handling in `remote_embedding.rs`): terminal (zero retries), with a Spanish error
     distinguishing whether `proxy.auth` was configured or anonymous, without ever logging
     credentials.
2. **Out of scope — crawl, discovery, sitemap, robots, and asset download paths:**
   `WreqDownloader`, `FetchRouter`, sitemap discovery, and MCP URL pre-checks fetch
   **untrusted, third-party URLs** discovered dynamically during a crawl. Routing untrusted
   crawl URLs through an internal corporate proxy turns that proxy into a *confused deputy*
   capable of reaching internal intranet hosts by hostname if the proxy's ACLs are
   permissive, and conflicts with `ADR-0018`'s WAF/egress architecture. All non-AI clients
   remain pinned to direct egress (`.no_proxy()`).

---

### Decision 4 — Observability: `webfang doctor`, `webfang show`, and sanitized `tracing` metadata

1. **`webfang doctor` and `webfang show` (`cli/commands/blocks.rs`):**
   - `print_proxy_posture` reports two facts clearly:
     1. Ambient environment variables (`HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY`) are
        **always ignored**.
     2. For the resolved provider: either **direct egress** (no proxy configured, resolver
        validates destination IPs) or the **effective configured proxy**:
        - origin (`<scheme>://<host>:<port>`, sanitized — never userinfo),
        - provenance (`proxy = "<id>"` from `[[proxies]]` vs inline table),
        - proxy network policy (`allow_loopback`, `restricted` vs `allowlist` CIDRs/hosts),
        - proxy auth source label (`none`, `env`, `keyring`, `encrypted_file` — secret
          never shown),
        - explicit notice that destination hostname DNS resolution is delegated to this
          proxy.
   - `webfang doctor` runs its live `build_embedding_provider` + `probe_dim` preflight
     through the configured proxy, verifying proxy auth resolution, proxy SSRF guard, and
     end-to-end tunnel reachability in one command.
2. **Structured `tracing` (English, compliant with `docs/src/data-flow.md` log policy):**
   - Provider startup and AI request spans record `proxy_enabled: bool` and, when enabled,
     `proxy_origin` (`scheme://host[:port]`) and `proxy_id` (when named).
   - Proxy credentials, `Proxy-Authorization` headers, and payloads never appear in logs or
     `--trace-file` JSONL.
3. **Disclosure docs (`docs/src/data-flow.md` and `docs/src/installation.md`):**
   - Updated in the implementation PR to state accurately: proxy **environment variables**
     are ignored everywhere; crawl egress is direct-only; AI provider egress supports an
     explicit, guard-checked `[[proxies]]` / `proxy` declaration in `config.toml` (with the
     destination-DNS delegation contract stated plainly), while keeping the loopback
     sidecar recipe as an alternative when needed (e.g. SOCKS/NTLM-only corporate proxies).
   - `scripts/check_data_flow_doc.sh` claims manifest updated in lockstep.

---

## 3. Trade-offs & Alternatives Rejected

| Option | Verdict | Why |
| :--- | :--- | :--- |
| **Type-separated `ClientSecurityProfile` + `[[proxies]]` catalog (Chosen)** | **Accepted** | Zero env magic; compile-time separation between proxy dial policy and destination redirect policy; reusable across `embedding` + `completion`; zero new Cargo dependencies; `zeroize`-backed proxy auth via `AuthSource`. |
| **Opt-in flag to honour `HTTPS_PROXY` env var** | **Rejected** | Breaks determinism across machines; leaks proxy topology to process lists if passed on argv; still requires `config.toml` for RFC1918 `allow_cidrs`/`allow_hosts`, creating a split two-place configuration; encourages `user:pass@host` in env vars. |
| **Pre-resolve `base_url` locally via `ValidatingResolver`, then `CONNECT`** | **Rejected** | Breaks mandatory corporate proxies on split-horizon DNS networks (no direct external DNS from workstations) and suffers from DNS-rebinding TOCTOU between client resolution and proxy resolution. |
| **Reuse `provider.network_policy` for both `base_url` and `proxy`** | **Rejected** | Security defect: allowlisting the proxy's RFC1918 subnet on the provider also opens `redirect_policy_with_policy` to follow upstream `302` redirects into that RFC1918 subnet. |
| **Enable `wreq` `socks` feature (`socks5` / `socks5h`) in v1** | **Rejected for v1** | `socks5` (local DNS) conflates proxy and destination resolution on `wreq`'s single resolver slot; `socks5h` adds crate/SBOM surface before any operator need beyond HTTP `CONNECT` is established. Can be added later for `socks5h` only if demanded. |

---

## 4. Implementation Slices & Acceptance Gates

To respect the ~500-line file ceiling (`scripts/check_file_size.sh`) and `ADR-0020`'s
sequential structure, implementation should land cleanly in focused units:

1. **Domain types (`domain/proxy_config.rs` + `domain/providers.rs` + `domain/ssrf_guard.rs`):**
   `ProxyConfig`, `ProxyAuth`, `ProxySpec` (`Named` | `Inline`), `ProxyRegistry`,
   `ValidatedProxy` (scheme `http`/`https`, no userinfo, literal-IP gate + FIN-017),
   `EndpointPolicy`, `ClientSecurityProfile`, and `SsrfGuard::secure_client_with_profile`.
   Unit-tested in domain (Miri-clean, no network).
2. **Infrastructure wiring (`infrastructure/ssrf.rs` + `infrastructure/llm/http_client.rs`):**
   `DefaultSsrfGuard::secure_client_with_profile` implementation (`.redirect(dest)` →
   `.no_proxy()` → `.proxy(...)` → `.dns_resolver(proxy_or_dest)`), shared LLM HTTP client
   builder (`ADR-0020` Slice 2), 407 classification, and symmetric wiring into both
   `RemoteEmbeddingAdapter` and `OpenAiCompatibleProvider` (plus `webfang_cli` / `llm_wire`).
3. **CLI diagnostics & disclosure docs (`cli/commands/{blocks,doctor,show}.rs`, `docs/src/{data-flow,installation}.md`):**
   Effective proxy reporting vs no-proxy posture, structured `tracing` fields, and
   `scripts/check_data_flow_doc.sh` verification.
4. **End-to-end harness (`crates/webfang_core/tests/proxy_posture_e2e_test.rs` / `ai_proxy_e2e_test.rs`):**
   `wiremock` acting as an observable local forward proxy proving:
   - ambient `HTTP_PROXY` / `HTTPS_PROXY` remains ignored (0 requests at ambient proxy),
   - configured loopback/RFC1918 proxy **without** proxy opt-in is rejected pre-socket (0 requests),
   - configured proxy **with** opt-in routes AI requests for an unresolvable external hostname through the proxy (`received_requests() == 1`),
   - client-side guarantees remain intact under proxy: literal-IP `302` redirect into the proxy's own subnet (or `169.254.169.254`) is stopped before a second proxy request, 16 MiB body cap triggers, and `429` backoff pacing fires before retry.
