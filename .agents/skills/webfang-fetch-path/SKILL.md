---
name: webfang-fetch-path
description: "Trigger: new fetch path, downloader, SSRF, retry, body cap, fetch guard chain. Wire a webfang fetch path through the mandatory guard order; the AGENTS.md table stays normative."
license: Apache-2.0
metadata:
  author: webfang
  version: "1.0"
---

## Activation Contract

Activate when adding or modifying any fetch path (CLI, engine, MCP tool, benchmark, new adapter),
wiring SSRF/retry/body-cap protections, or reviewing a fetch path for guard-order compliance.
Skip for non-network code.

## Hard Rules

- Replicate the mandatory order, every fetch path: ValidUrl (entry) → rate limit (pacing) → per
  attempt: timeout → redirect policy → SSRF at the socket dial (every hop) → retry classification
  → body read. Deviating from the order is a bug, not a style choice.
- Never connect before validating; never pace after dialing (a doomed request burns rate budget
  the SSRF guard would have rejected). Each redirect hop re-enters the chain — a new dial.
- HTTP is always `wreq`, never `reqwest` (TLS fingerprint impersonation for WAF evasion).
- No fetch path may read an uncapped body; cap at `DEFAULT_MAX_PAGE_BYTES`, `BodyTooLarge` beyond it.
- Retry classification runs before the body read; report the last observed status when retries
  exhaust, never a hardcoded one; WARN only when a retry actually happened.
- If a path cannot reuse `WreqDownloader`/`FetchRouter`, the order must still hold; cite the
  AGENTS.md fetch-guard table in the review.

## Decision Gates

| Situation | Action |
| --- | --- |
| New fetch path can reuse the downloader | Wire through `WreqDownloader`/`FetchRouter`; stages already ordered |
| New adapter cannot reuse it | Replicate stages 1→4 before stage 5 by hand; cite the table in review |
| Redirect target is a forbidden literal IP | Synchronous stop (`redirect_policy`); SSRF re-checks at the dial |
| 403 with pinned UA | No UA rotation; report as-is |

## Execution Steps

1. Validate the entry URL into `ValidUrl`/`try_from_url` at the argv boundary.
2. Pace before the network (token bucket / `RateLimiterConfig`); a cancelled wait counts as skipped.
3. Wire timeout + redirect policy + SSRF-at-dial (every hop) per attempt.
4. Classify retries before reading the body (transients/429/5xx/403/timeout rules).
5. Read the body capped (`read_body_capped`); cite the AGENTS.md table in the review.

## Output Contract

Return: the fetch path wired, the stage-by-stage order checklist (1→5) with the reference symbol
for each, confirmation of `wreq` (not `reqwest`) and a capped body read, and the review citation.

## References

- `references/sources.md` — normative table + symbol map (current paths).
- `../../../AGENTS.md` — § Fetch guard chain (MANDATORY ORDER table, normative).