# Mission: proxy posture — issue #2500

Resolution: **declare proxy support unsupported, explicitly.**

## Specs

- S#1: "Does `wreq` honour `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` at all?" — answered in code
  (2026-10-30): YES by default (`auto_sys_proxy: true`, `ProxyMatcher::system()` pushed in
  `ClientBuilder::build()`; no webfang client calls `.no_proxy()`).
- S#2: "either the behaviour is implemented and tested, or the absence is documented with a
  workaround" — we pick the **absence** branch: env proxies are explicitly disabled so behavior
  is deterministic and the SSRF resolver keeps resolving the real destination.
- S#3: "`docs/src/data-flow.md` states the proxy posture for the remote embedding endpoint,
  next to the egress destinations it already lists."
- S#4: "`webfang doctor` either reports the effective proxy or explicitly says there is none."
- S#5 (code-trace finding, motivates S#2): under a proxy the validating SSRF resolver resolves
  the PROXY host, not the destination — an RFC1918 corporate proxy would be rejected by the
  guard, and destination IP checks would be delegated to the proxy. Both unacceptable silently.
- S#6: user instruction — "no te fíes en la doc, fíate en el código": the current honored-env-proxy
  behavior must be pinned by an observable test BEFORE the change (RED), then flipped (GREEN).

## Tasks

- T1: RED characterization test — with `HTTPS_PROXY`/`ALL_PROXY` pointing at a wiremock, a
  scrape reaches the target through the proxy today (observable: proxy receives the request).
- T2: GREEN — `.no_proxy()` at the single client-construction choke point (the guard's
  `secure_client*` impls, `infrastructure/ssrf.rs`), with a comment citing this issue.
- T3: `webfang doctor` prints the explicit proxy posture (none; env ignored by design).
- T4: docs — `data-flow.md` egress section + `installation.md` AI section: posture, rationale,
  relay/sidecar workaround.
- T5: gate — check, clippy (all-features), fmt --check, nextest affected, snapshots reviewed.

## Log

- L1: user request — "issues #2500 (priority:medium) y #1817. revisar y analizar" → analysis;
  then "arrancamos con #2500 pero no te fies en la doc, fiate en el codigo".
- L2: wreq code trace (see S#1, S#5). Open product question asked; no answer → proceeded with
  recommended smallest-safe slice (S#2). Guard-aware proxy support = future issue, not here.
- L3: worktree `fix-proxy-posture` @ main 4433fc67; indexes OK; mission file created pre-write.
