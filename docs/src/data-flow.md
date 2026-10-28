# Data Flow & Privacy

This page is the **data-flow disclosure**: what leaves your machine when you
run webfang, to which destinations, under what conditions, what is stored
locally, and how to turn every flow off. Everything stated here is enforced by
`scripts/check_data_flow_doc.sh` against the source tree, so it cannot silently
drift from the code.

---

## Quick path: run with zero AI egress

1. Do not pass `--clean-ai` or `--extract-with-llm`, and do not use the MCP AI
   tools — no AI request is ever made without an explicit flag or tool call.
2. Do not configure an AI provider in the providers config — without one,
   an AI request fails loudly at startup with exit 78, it never falls back to
   a remote default.
3. Run with `--offline` on cleaning paths — every embeddings provider
   (remote **or** loopback) is rejected, exit 78, before any endpoint is
   inspected.
4. Scraped-site traffic is inherent to crawling: fetching the pages you asked
   for **is** the product. Restrict it with the crawl scope flags; every
   destination is checked by the SSRF guard before a socket exists.

---

## What leaves the machine

| Destination | When | What is sent | How to turn it off |
| :--- | :--- | :--- | :--- |
| The site being scraped | Every crawl/scrape | HTTP GET requests for the pages (and, with asset download, for images/documents) — standard browser-like fetches via the TLS-fingerprinted client | Omit the URL(s); scope with crawl flags |
| The operator-configured LLM endpoint (`POST {base_url}/chat/completions`) | `--extract-with-llm` | Extracted text chunks plus the extraction schema | Omit the flag; or configure no completion provider (exit 78 at startup) |
| The operator-configured embeddings endpoint (`POST {base_url}/embeddings`) | `--clean-ai` (and the MCP `semantic_cleaner` tool) | Extracted text chunks (bounded by `--max-chars`) | `--offline`; or configure no embedding provider |

There is **no other network destination**: no telemetry, no metrics, no
update check, no crash reporting. Every HTTP fetch in the product goes through
the SSRF-guarded downloader or through the two OpenAI-compatible AI adapters
above. (The historical Hugging Face / local-ONNX paths were removed — see
[the ONNX migration page](ai-onnx-migration.md).)

---

## The payload contract: extracted text, never raw HTML

The remote model never receives your raw page source. Before any text is sent
to an AI endpoint it passes through the text-extraction stage:

- The semantic cleaner extracts readable text from the page HTML — first a
  legibility parse, falling back to a plain tag-strip — and then chunks that
  text under the `--max-chars` budget. This is the invariant stated in the
  orchestrator: **clean (raw HTML never reaches the LLM)**. Implementation:
  `application/llm_extraction.rs`, and
  `infrastructure/semantic_cleaner.rs::extract_text_content`.
- The structured-extraction path (`--extract-with-llm`) composes the same
  cleaner with the chunker, so the completion payload is chunked extracted
  text plus your schema — never the page's HTML.
- Authentication: requests carry an `Authorization: Bearer` header **only** if
  the provider config supplies a credential; a provider with
  `auth: { source: "none" }` sends an anonymous request. Credentials resolve
  locally at startup; they are never logged.

Endpoint reachability is enforced by the same fetch-guard chain used for
crawling (entry validation → timeouts → redirect policy → SSRF at the socket
dial → retry classification → capped body read). Loopback embedding endpoints
are rejected unless explicitly opted in, and `--offline` rejects every
provider before the endpoint is even inspected.

---

## Log and trace content policy

The observability stack (structured logs on stderr, the `--trace-file` JSONL)
carries **operational metadata only**:

- **What you will find:** timestamps, levels, span and target names, URLs,
  depths, HTTP statuses, durations, pacing waits, error messages with their
  stage, and correlation identities. The [debugging guide](debugging.md)
  documents every span and field.
- **What never appears:** page bodies, HTML, extracted Markdown, chunk text,
  or provider credentials — no `tracing` field in the product crates carries
  page content. The JSONL is identifiers and outcomes, not payloads.
- **MCP responses:** every tool result that carries third-party text is
  neutralized (ANSI/control characters stripped), byte-capped with a visible
  truncation marker, and wrapped in a nonce-delimited UNTRUSTED provenance
  envelope. See `docs/security/prompt-injection-policy.md` for the consuming
  rules.
- Trace files are local, ordinary files at the path **you** choose with
  `--trace-file`. They are never uploaded anywhere; delete them whenever you
  like.

---

## Local retention

| Path (Linux / XDG) | What lives there | Created by | Removal |
| :--- | :--- | :--- | :--- |
| `~/.webfang/crawl.db` | SQLite database (crawl results / RAG export) | Any run that persists results; override with `WEBFANG_DB_PATH` / `--db-path` | Delete the file |
| `~/.cache/webfang/state/` | Export state JSON plus the JSON-per-domain record store (one versioned envelope per site) | Resume/checkpoint runs; override with `--state-dir` | Delete the directory |
| `<run persistence root>/.webfang-batch-capture.jsonl` | The **batch spool**: every page body the batch fetched, verbatim | Batch runs, until the batch finishes and it is discarded | Discarded at the end of a healthy batch; a crashed run leaves it, and the next batch run sweeps stale spools — or delete it by hand |
| `~/.config/webfang/` | `config.toml` and the encrypted credential store `credentials.age` | Configuration and `webfang connect` | Delete the directory (also part of uninstall) |
| `output/` (under your working directory) | Exported Markdown/JSON and downloaded assets | Every run that exports or downloads | Delete at will |
| Your `--trace-file` path | The JSONL trace (metadata only, see above) | Runs with tracing enabled | Delete at will |

Windows/macOS use the platform equivalents documented in
[installation.md](installation.md); the paths above follow the standard
XDG conventions the platform-paths helper enforces.

SQLite state and the WAL sidecar files (`-wal`/`-shm`) have their own
recovery procedure: see the [State Recovery Runbook](state-recovery-runbook.md)
before deleting a database you still need.

---

## Review checklist

- [ ] I know every destination my run can talk to (the table above) and none
      of them is hard-coded by webfang.
- [ ] I can run a full crawl with **zero** AI egress (Quick path above).
- [ ] I understand the AI payload is extracted text chunks, never raw HTML.
- [ ] I know where every retained file lives and how to remove each one.

<!-- claims:begin
```claims
# The manifest scripts/check_data_flow_doc.sh audits against the tree.
# contains | <literal>          — this doc must name the literal somewhere
# code     | <path> | <symbol>  — the cited file must exist and contain it
contains | never raw HTML
contains | chat/completions
contains | embeddings
contains | --extract-with-llm
contains | --offline
contains | telemetry
contains | .webfang-batch-capture.jsonl
contains | crawl.db
contains | credentials.age
contains | state-recovery-runbook.md
code | crates/webfang_core/src/application/llm_extraction.rs | raw HTML never reaches
code | crates/webfang_core/src/infrastructure/semantic_cleaner.rs | extract_text_content
code | crates/webfang_core/src/infrastructure/llm/client.rs | chat/completions
code | crates/webfang_core/src/infrastructure/llm/remote_embedding.rs | /embeddings
code | crates/webfang_core/src/cli/llm_wire.rs | build_llm_provider
code | crates/webfang_core/src/cli/batch_flow.rs | .webfang-batch-capture.jsonl
code | crates/webfang_core/src/infrastructure/autotuning.rs | crawl.db
code | crates/webfang_core/src/domain/local_secret_store.rs | credentials.age
```
claims:end -->

