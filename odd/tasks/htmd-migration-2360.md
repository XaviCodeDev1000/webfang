# Mission: htmd-migration-2360

Branch: `chore/htmd-migration-2360` · Issue: #2360 (status:approved) · PR base: `main`

## Specs

- S1: "#2360 — migración a htmd. Va acá porque toca Cargo.toml/Cargo.lock (mayor blast radius de review) y conviene que los fixes de producto ya estén en main." (Fase 4 — Chores)
- S2: "ten cuidado al hacer builds ejecutando comandos pesados, porque hay otros agentes trabajando" (Fase 4 — Chores)
- S3: Issue #2360 body is the authoritative spec: migrate html-to-markdown-rs → htmd, MarkdownConverter port (domain) + HtmdConverter adapter (infrastructure), html_cleaner stays as pre-pass, collapse duplicated fallbacks, remove html-to-markdown-rs, drop lru 0.16.4 + html5ever 0.36 duplicates, RUSTSEC-2026-0253 ignore no longer needed, zero new duplicates, no new licenses, snapshot diffs reviewed 1-by-1, MCP/scrape contract preserved.

## Tasks

- T1: Explore converter surface (callers of convert_to_markdown, both fallbacks, snapshots affected).
- T2: Introduce MarkdownConverter port in domain + HtmdConverter in infrastructure (closest-pattern).
- T3: Rewire scrape markdown path + MCP convert_html_to_markdown onto the port.
- T4: Collapse domain/scraper_port.rs::fallback and infrastructure/scraper/fallback.rs onto the single converter.
- T5: Remove html-to-markdown-rs from Cargo.toml; regenerate Cargo.lock.
- T6: Snapshot diffs reviewed one by one; classify regression vs acceptable.
- T7: cargo deny check + scripts/check_advisory_policy.sh + prune dead duplicate-baseline entries.
- T8: Full pre-commit gate (check, clippy --all-features, fmt --check, doc, nextest affected) + PR.

## Log

- L1: Fase 4 assignment (2026-10-29): migrate #2360 now; builds via mbx, sequential, watch other agents.
