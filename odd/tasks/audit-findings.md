# ODD feature: audit-findings — fix the actionable 2026-11-23 audit findings

Objective: open issues for the audit's actionable findings and fix the ones that are safe and unambiguous; leave API/semver and platform decisions as decision issues. Branch `fix/audit-findings` from `a3c6654c`; delivery `ask-on-risk`, work-unit commits on the feature branch. Test runner: `cargo nextest run --workspace --all-features` plus the four-command pre-commit gate.

## Specs

- **S1 — credential redaction (issue #3006).** User's checklist string: "Comprobar redacción de credenciales en logs, --trace-file, doctor, show y mensajes de error (REDACTED_CREDENTIAL)". Acceptance: a regression test through the public `show`/`doctor` surface proves that userinfo in a provider `base_url` is never rendered while the endpoint remains identifiable; no egress behavior change. Check: focused nextest target + `cargo check`.
- **S2 — benchmark honesty (issue #3007).** User's checklist string: "Ejecutar pruebas de carga/concurrencia midiendo latencia, memoria, colas y estabilidad". Scope limited to the audited defect: the RAM column reports a declared constant. Acceptance: the metric is named for what it is, deterministic report/cost-model tests stay green, and Tier B deferral stays documented (no fake execution). Check: `cargo nextest run -p webfang_benchmark`.
- **S3 — poisoned mutex recovery (issue #3008).** User's checklist string: "Auditar invariantes // SAFETY: en bloques unsafe, ausencia de locks sostenidos en .await y timeouts en toda operación de red/I/O". Acceptance: every production lock site in `ResolutionTracker` recovers with `PoisonError::into_inner()`; a deterministic test poisons a local tracker and proves the mark survives. Check: module-focused nextest.
- **S4 — embedding content boundary (issue #3009).** User's checklist string: "Confirmar que retos WAF no disparen escalados indebidos y que no se envíe HTML crudo a proveedores LLM/embeddings". Acceptance: the degraded HTML stripper used before embedding drops script/style/noscript bodies; a regression test forces the degraded path. Check: focused nextest + `cargo check`.
- **S5 — panicking trait default (issue #3010, decision only).** User's checklist string: "Auditar contratos de webfang_core / puertos de dominio / adaptadores y compatibilidad interna/externa". Acceptance: issue documenting the semver choice (required method vs deprecation-first); no code change until the owner decides. Check: issue exists and links the evidence.
- **S6 — doc drift (issue #3011).** User's checklist string: "Documentar fallos preexistentes conocidos en tests para no confundirlos con hallazgos". Acceptance: `deny.toml` advisory comment and AGENTS.md WAF error-class claim match current sources of truth; no gate semantics change and no `docs/` edit. Check: readback + `cargo deny --offline check`.
- **S7 — Cloudflare feasibility (issue #3012, decision only).** User's checklist string: "Mapear dependencias externas (navegadores, subprocesos, FS, keyring, bases locales) para ejecución en plataforma". Acceptance: decision issue with the verified OS couplings; no code change. Check: issue exists.
- **S8 — regression proof and final matrix.** User's checklist strings: "Crear prueba de regresión por cada hallazgo corregido y verificar dry-run limpio desde cero" and "Preparar matriz final de estado (36 herramientas MCP + CLI + API + módulos) para la reunión con Cloudflare". Acceptance: one named regression test per fixed finding; full suite green; matrix delivered in the final report (no new repo file outside allowed surfaces). Check: full nextest + full pre-commit gate.

## Tasks

- T1 — S1 — inline — fix(cli): redact provider base_url userinfo — issue #3006 — commit: 2766f8e7
- T2 — S2 — inline — refactor(bench): honest RAM metric naming — issue #3007 — commit: c2c949c5
- T3 — S3 — inline — fix(domain): recover poisoned ResolutionTracker lock — issue #3008 — commit: 66cd48e4
- T4 — S4 — inline — fix(ai): drop script/style text in fallback stripper — issue #3009 — commit: cc7dbff0
- T5 — S5 — inline — decision issue for EmbeddingPort::as_any (no code) — issue #3010 — delivered as issue
- T6 — S6 — inline — docs: correct stale advisory/WAF claims — issue #3011 — commit: a5fdf010
- T7 — S7 — inline — decision issue for Cloudflare portability (no code) — issue #3012 — delivered as issue
- T8 — S8 — inline — regression proof, full gates, final matrix — issues #3006-#3009 — commit: e51c5a32 (fail-closed follow-up) + this record

## Log

**L1 (verbatim):** "abrir las issues para solucionar lo que se debe arreglar y solucionar"

**L2 — 2026-11-23, classification evidence.** Read-only root-cause map (Engram obs #3322): FIX-NOW = #3006 base_url redaction, #3007 RAM naming, #3008 mutex recovery, #3009 script/style stripping, #3011 doc drift; ISSUE-ONLY = #3010 public trait semver, #3012 Cloudflare portability; ACCEPT-AS-DESIGNED = release panic probe and runtime SSRF kill-switches (both documented, test-pinned, default-off/load-bearing). Cluster rule applied: one issue and one fix per root cause, not one patch per symptom.

**L3 — 2026-11-23, baseline.** Worktree `fix-audit-findings` at `a3c6654c`; `.envrc` (5 lines, with `export` on lines 1/2/4 — the parser requires it), direnv allowed, identity gate exit 0, codegraph 956 files and codedb 1683 files both rooted in the worktree, seed `cold` (normal), `cargo check --workspace --all-features` exit 0 in 57s.

**L4 — 2026-11-23, decisions taken inline (no product scope expansion).** S1 fixes only the diagnostic leak; load-time userinfo rejection is explicitly out of scope (config-format change). S2 renames the misleading metric instead of adding a dependency or platform-specific RSS read. S5 and S7 are owner decisions and stay as issues. RDD is on (global); native review will assess each work-unit commit.

**L5 — 2026-11-23, verification evidence.** Final state `e51c5a32`: `cargo check --workspace --all-features`, clippy with the full gate flags, `cargo fmt --all -- --check`, `env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps` all exit 0; `INSTA_UPDATE=no cargo nextest run --workspace --all-features` → 5754 passed, 8 skipped. Named regression tests: `show_and_doctor_redact_provider_base_url_userinfo`, `show_and_doctor_never_print_unredactable_base_url_text`, `ram_metric_is_labeled_as_declared_estimate`, `a_poisoned_mark_mutex_still_records_and_clears_marks`, `degraded_stripper_drops_script_style_noscript_before_embed_batch`. Each observed RED before GREEN.

**L6 — 2026-11-23, native review blocked (not a pass).** Candidate `2766f8e7` was assessed high-risk, consent `granted` was given for this candidate only, lineage `review-3df1429f67b3b29a`. Three lens captures were admitted (risk, readability, reliability) and produced one severe finding R3 (ignored `set_username`/`set_password` results on cannot-be-a-base URLs). The provider refuter failed twice with "Provider response headers timed out after 300000ms"; the bound STATUS reoffered the same slot but no verdict was fabricated. Because the failure came from the delegated subagent transport, the provider-defect classification rule says no report and no handoff. R3 was nevertheless reproduced (RED: raw `mailto:user:s3cr3t-do-not-print@host/v1` rendered) and fixed in the follow-up commit `e51c5a32` with a snapshot proving `<redacted provider url>`. Review authority for `2766f8e7` remains unacknowledged; no review PASS is claimed.

**L7 — 2026-11-23, delivery state.** Seven work-unit commits on `fix/audit-findings`; no push, PR or merge (owner decisions). Issues #3006-#3009 are fixed pending approval; #3010 and #3012 are decision-only; #3011 docs fixed. Final matrix delivered in the chat report, not as a new repo file.
