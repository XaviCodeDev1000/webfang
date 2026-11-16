# Sources — webfang-fetch-path

This skill is the actionable layer. The MANDATORY ORDER table is normative in `AGENTS.md` § Fetch
guard chain — this skill guides applying it, never replaces it. Cite the symbol, not a line number.

## Repo sources of truth (normative)

- `../../../AGENTS.md` — § Fetch guard chain: the stage table (entry validation → pacing →
  timeout → redirect policy → SSRF at dial → retry classification → body read) and the "why" for
  each stage. This is the authority for the order.

## Symbol map (current paths, verified via codedb)

| Stage | Symbol | Path |
| --- | --- | --- |
| 1 Entry validation | `ValidUrl` / `try_from_url` | `crates/webfang_core/src/domain/value_objects.rs` |
| 2 Pacing (scrape) | `scrape_urls` | `crates/webfang_core/src/cli/scrape_flow.rs` |
| 2 Pacing (bucket build) | `build_scrape_rate_limiter` | `crates/webfang_core/src/application/batch/processor.rs` |
| 2 Pacing (cancel-aware) | `until_ready_or_cancel_observed` / `RateLimiterConfig` | `crates/webfang_core/src/application/rate_limiter.rs` |
| 3a/4 Timeout + retry | `WreqDownloader::fetch_inner` | `crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs` |
| 3b Redirect policy | `redirect_policy` | `crates/webfang_core/src/domain/ssrf_guard.rs` |
| 3c SSRF at dial | `ssrf_guard` | `crates/webfang_core/src/domain/ssrf_guard.rs` |
| 5 Body read | `read_body_capped` (generic cap helper) | `crates/webfang_core/src/domain/body_cap.rs` |
| 5 Cap constant | `DEFAULT_MAX_PAGE_BYTES` | `crates/webfang_core/src/domain/downloader_factory.rs` |

`fetch_inner` also appears in `application/http_client/client.rs`; the downloader method is the
reference implementation cited by the AGENTS.md table. Re-resolve with `codedb "$PWD" symbol <name>`
if the tree moves.

## Engram (related context)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| Support line routing / hotfix (fetch fixes routing) | `support line routing hotfix branch taxonomy versioning` | #2308 | project |

Resolve with `engram search "<query>" --project webfang` or `mem_search`.