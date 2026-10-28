# data-flow-disclosure — doc de divulgación de flujo de datos (#2241)

**Branch:** `docs/data-flow-disclosure` (worktree `docs-data-flow-disclosure`)
**Issue:** #2241 (Closes part of #1609) · labels: `priority:medium` `status:approved` `type:docs`

## Specs

Del issue #2241 (verificado contra `main` 052b805e):

- S1 — "`docs/src/data-flow.md` exists, is listed in `docs/src/SUMMARY.md`, and states: (a) which data leaves the machine, to which destinations, when, and how to turn it off; (b) the payload contract — extracted/chunked text, **never raw HTML**; (c) the egress hosts as they are today (scraped sites + assets + the operator-configured remote endpoint); (d) the log/trace content policy: what never reaches the JSONL nor the MCP stderr; (e) local retention, including `~/.webfang/crawl.db` and the batch spool."
- S2 — "The retention section either contains the WAL/SQLite runbook or links to the doc that contains it." → decisión: enlazar `docs/src/state-recovery-runbook.md` (ya existe).
- S3 — "`scripts/check_data_flow_doc.sh` asserts the doc names the payload and the destinations, and `scripts/test_data_flow_doc.sh` covers it (the pattern of `scripts/check_ai_providers_doc.sh`)."
- S4 — "The check is wired into CI."
- Nota del issue: el draft del audit está OBSOLETO (menciona HF/`HF_ENDPOINT`, ONNX, feature `ai`) — reescribir contra el árbol actual, nunca levantar tal cual.

## Facts verificados contra el árbol (evidencia)

- Payload: `application/llm_extraction.rs` (doc de módulo: "clean (raw HTML never reaches the LLM)"); `infrastructure/semantic_cleaner.rs::extract_text_content` (legible → fallback tag-strip).
- Egress: sitios scrapeados + assets; endpoints OpenAI-compat configurados por el operador — `POST {base_url}/chat/completions` (`infrastructure/llm/client.rs`), `POST {base_url}/embeddings` (`infrastructure/llm/remote_embedding.rs`, Bearer sólo con credencial, `auth: {source:"none"}` anónimo); loopback rechazado sin opt-in; `--offline` rechaza todo con exit 78 (`application/crawl_options.rs`).
- Trazas: JSONL lleva URLs/estados/duraciones/identidad (`docs/src/debugging.md`); rg sobre macros de tracing con campos `body|html|content|markdown` = 0 hits en crates de producto; resultados MCP neutralizados + envelope de provenance (`mcp_server/provenance.rs`).
- Retención: `~/.webfang/crawl.db` (`infrastructure/autotuning.rs` `DEFAULT_DB_DIR`/`DEFAULT_DB_FILE`, override `WEBFANG_DB_PATH`); state dir `<cache>/webfang/state` (`cli/scrape_flow.rs::resolve_default_state_dir`) con record store JSON-por-dominio v2 (`infrastructure/export/record_store.rs`) y ExportState; spool de batch `.webfang-batch-capture.jsonl` bajo el persistence root del run, con cada cuerpo verbatim y sweep best-effort (`cli/batch_flow.rs`, DF-L1 #1615); config `<config>/webfang/` + `credentials.age` (`domain/local_secret_store.rs`); output `output/`; trace file a elección del operador.

## Tasks

- T1 — escribir `docs/src/data-flow.md` (S1, S2) + entrada en `docs/src/SUMMARY.md`.
- T2 — `scripts/check_data_flow_doc.sh` (guard fail-closed, patrón ai-providers) (S3).
- T3 — `scripts/test_data_flow_doc.sh` con marcador `# NEGATION:`; wiring assertions incluidas (S3).
- T4 — wiring en `.github/workflows/ci.yml` + `scripts/ci_fast_gate.sh` (S4) — el issue aprobado lo exige.
- T5 — meta-guard: `bash scripts/test_guard_coverage.sh` verde (el guard nuevo debe estar wired + harness que muera con el guard stubbeado).
- T6 — gates, commits por unidad de trabajo, push, PR `Closes #2241` + `Closes part of #1609`.

## Log

- L1 — (2026-10-28) Usuario: "Issue #1609 revisa y analiza en profundidad su contenido pero debes ahorrar recursos cuando haces builds para no saturar los recursos del sistema" → análisis read-only; luego "adelante, si es informacion confirmada y segura" → autorización para ejecutar #2241.
- L2 — Verificación de hechos completada (ver arriba). Sin builds: todo el trabajo es docs + bash.
- L3 — Restricción de recursos: cero `cargo build`; verificación con guards bash + `ci_fast_gate` lane docs.
