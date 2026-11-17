# batch-input-reconciliation

Reconcile discarded batch-input lines in the final summary, with line numbers and `--strict-input` (#2795).
Branch: `feat/batch-input-reconciliation` @ worktree `feat-batch-input-reconciliation`, base `main` @ d4cc9de0.
Delivery: `ask-on-risk` (single PR planned; push/PR is a maintainer decision). Test runner: `cargo nextest run`.

## Specs

Quote-verbatim requirements from issue #2795 (maintainer, `status:approved` 2026-11-14) and its T3 characterization comment.

- **S1** — "Un archivo con 4 líneas (2 válidas, 1 inválida) produce un resumen que **concilia** las tres cifras: leídas, válidas y descartadas." Proposed format: `Batch complete: 2/3 succeeded, 1 failed — 37 lines read, 4 valid, 1 rejected`.
- **S2** — "El aviso por línea lleva el **número de línea** de la entrada." Proposed: `batch line rejected by URL entry validation, line: 31, reason: ...`.
- **S3** — "Con `--strict-input`, un archivo con alguna línea inválida **falla** con un mensaje que las cuenta y con exit code de error de configuración (78), en vez de continuar."
- **S4** — "La documentación de `--batch-file` declara el comportamiento de descarte (hoy no lo declara: solo dice \"Path to a file containing URLs to scrape (one per line)\")."
- **S5** (T3) — "las líneas vacías deben contar como descartadas (o como una categoría propia, si se decide distinguirlas)". DECIDED: own categories — blank lines and `#` comments are legal input and must not be reported as "rejected"; the arithmetic `lines read = valid + rejected + blank + comment` must close.
- **S6** (T3) — "Un archivo sin ninguna URL válida debe decir **cuántas líneas se leyeron y por qué se rechazaron**, no solo \"No URLs provided\"."
- **S7** (issue Note, invariant) — discard itself is intentional (F-R3-1 keeps credential-looking lines out of logs/traces/processor); the fix is operator reconciliation, NOT removing the discard. The per-line warning must never echo the offending line text.

## Design decisions (analysis 2026-11-17, delivered to maintainer and accepted)

- `BatchInputLines` stats struct in the batch manager: `lines_read / valid / rejected / blank / comment` + `Vec<(line_number, reason)>`. Built by `parse_url_lines` via `enumerate` over `content.lines()` (1-based physical line numbers; `from_file` reads the file whole, so numbers are exact).
- `BatchManager` retains the stats (`Option<BatchInputLines>`: `None` when built from CLI URLs); `BatchManagerSummary` carries them so `log_batch_summary` can print the reconciliation suffix whenever `lines_read != valid` (silent when everything was processed — no noise on clean files).
- `--strict-input` enforcement at `load_batch_manager` (covers batch run AND dry-run): when `stats.rejected > 0` → `CliExit::ConfigError` = exit 78 with a Spanish message counting lines. Exit-78 justification: policy-flag precedent `--offline` (#2166) and sysexits EX_CONFIG ("unconfigured or misconfigured state"); non-strict invalid URLs stay exit 64 (matrix row 15) — this is a new CLI-boundary policy exit, not a reclassification; no new `ScraperError` variant, so no matrix-doc change.
- Blank/comment lines never trip `--strict-input` (only `rejected` does) and never WARN per line (legal input).
- `--batch-file` help text is edited in the `OptionSpec` table (`domain/options_spec/`) — single source that feeds clap via `spec_command`; parity tests enforce it.

## Tasks

- **T1** (S1,S5) — stats struct + `parse_url_lines` rework + manager/summary fields + reconciliation suffix in `log_batch_summary`. Route: inline. Risk: none. Commit: `ea0fe996`.
- **T2** (S2,S7) — `warn_skipped_lines` carries `line:` (structured tracing field, English; reason text stays Spanish error Display). Route: inline. Risk: none. Commit: `ea0fe996`.
- **T3** (S3) — `--strict-input` surface (OptionSpec const + spec_command registration + `ExportArgs` field + `From<Args>`→`BatchOptions`) + enforcement → exit 78. Route: inline. Risk: none. Commit: `ea0fe996`.
- **T4** (S6) — "No URLs provided" error message reports lines read / valid / rejected (+ blank/comment), keeps exit 64. Route: inline. Risk: none. Commit: `ea0fe996`.
- **T5** (S4) — `BATCH_FILE` OptionSpec help declares absolute http(s) URLs, blank/comment handling and discard-with-line-number behavior. Route: inline. Risk: none. Commit: `ea0fe996`.
- **T6** — full gates (`cargo check`, `clippy --all-targets --all-features -D warnings`, `fmt --all -- --check`, rustdoc, nextest affected, `ci_fast_gate.sh` with coverage read) + `cargo insta review` of every snapshot diff. Route: inline. Risk: none. Commit: `ea0fe996`.

Regression tests (behavioral, `tests/behavioral/cli/batch_test.rs` conventions, wiremock + TempDir + snapshots):
- R1 (S1/S5): 5-line file (2 valid, 1 valid→HTTP 500, 1 relative, 1 blank) → summary snapshot reconciles 4 categories.
- R2 (S2): WARN carries `line: N` matching the physical line; no offending text echoed.
- R3 (S3): `--strict-input` with a rejected line → exit 78 + counting message; without the flag → exit 0/continues; blank+comment-only differences never trip it.
- R4 (S6): all-blank/all-invalid file → error message with counts, exit 64.

## Log

- **L1** — "Issue #2795 vamos a analizarla y consultar la documentacion oficial antes de implementar" (2026-11-17).
- **L2** — Analysis delivered; maintainer accepted design (own categories for blank/comment; exit 78 for `--strict-input` with `--offline` precedent). Reply: "adelante con la implementacion".
- **L3** — Worktree `feat-batch-input-reconciliation` created off `origin/main` @ d4cc9de0; bootstrapped (.envrc 5 lines + seed `cold` + codegraph/codedb indexed). Feature doc created before first source write.
- **L4** — RED observed: all 6 new behavioral tests FAIL (3 tries each, deterministic) against main's behavior. One RED-time insight: the strict-input error message wording differs from the first assertion draft — the message is the spec (`--strict-input activo: la entrada tiene líneas rechazadas (líneas leídas: N, ...)`) and the assertion was aligned to it.
- **L5** — Implementation complete: `BatchInputLines` (+ re-export) in manager.rs, `parse_url_lines` accounts every physical line with 1-based numbers, `warn_rejected_lines` carries `line:`, summary carries `input_lines`, `log_batch_summary` reconciles when `lines_read != valid`, no-URLs error counts (exit 64 stays), `--strict-input` enforced in `load_batch_manager` (covers dry-run too) → exit 78, flag wired end-to-end (OptionSpec STRICT_INPUT + GROUP + ExportArgs + From<Args> + BatchOptions), BATCH_FILE help rewritten. Focused GREEN: batch behavioral 24/24.
- **L6** — Verification: behavioral+args+config_default+gate3 265/265, MCP 648/648 (spec parity accepted STRICT_INPUT without MCP changes), pre-commit gate GREEN after two fixes observed: clippy `field_reassign_with_default` (summary now built with struct-update syntax) and one rustfmt wrap (fixed via bare `cargo fmt`, verified with `--check`). Diff: 9 files, ~552 insertions / 46 deletions; CHANGELOG.md and docs/ untouched; zero pending `.snap.new`.
- **L7** — Final continuation verified the worktree identity, reviewed the full diff, normalized formatting/rustdoc findings, refreshed the intra-crate-lint golden fixture to 2,269 sites, and observed GREEN for `cargo fmt --check`, `cargo check`, clippy all-targets/all-features, rustdoc, 18/18 batch behavioral tests, and 23/23 intra-crate-lint tests. The long local-gate run reached the integration suite but exceeded the command window, so its result is recorded as partial rather than a product failure. Commit `ea0fe996` (`fix(cli): reconcile batch input accounting`) contains the verified implementation; push/PR remains a maintainer decision.
