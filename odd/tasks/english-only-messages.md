# English-only user-facing messages

Remove the Spanish-language convention for user-facing messages and translate the
in-scope Spanish text of the repository to English. Branch `feat/english-only-messages`.
Delivery: `ask-on-risk` (breaking-surface decision belongs to the maintainer).
Test runner: `cargo nextest run` + `cargo insta review` + `bash scripts/ci_fast_gate.sh`.

## Specs

- **S1 — Remove the convention.** Owner: *"Quiero eliminar esta convencion y utilizar
  unicamente English a partir de ahora."*
  Acceptance: no normative artifact states a Spanish-language rule for user-facing
  messages. `AGENTS.md` (3 sites: § Project rules, § Error stratification, pre-PR
  checklist), `README.md` (§ Architecture rules), `typos.toml` (Spanish allowlist),
  `.github/ISSUE_TEMPLATE/*.yml` ("Podés escribir el cuerpo en español") and the
  `# STYLE: user-facing errors are Spanish` comments in `scripts/` no longer encode it.

- **S2 — Translate the Spanish text of the repository.** Owner: *"necesito un plan para
  traducir todos los textos en espanol al English que estan en el repo."*
  Acceptance: zero Spanish-bearing lines in the in-scope areas, measured with the same
  high-precision detector used for the inventory (accented characters plus Spanish
  function words), and the full test suite green.

- **S3 — Confirmed scope: "Producto + docs (recomendado)".** Owner selected the product
  and documentation scope, which covers: `crates/**` (production `src`, `tests`, `.snap`),
  `scripts/**`, `typos.toml`, `.github/**`, `AGENTS.md`, `README.md`, `docs/**` (14 files)
  and `SUPPORT.md` (regenerated from `.github/support.json`, never hand-edited).
  **Excluded and left untouched as frozen history:** `evidence/**`, `odd/**`,
  `CHANGELOG.md` (single writer: the Release PR).

- **S4 — [Assumption, maintainer decision pending] Breaking surface.** User-facing CLI
  error strings are observable output: changing them is a breaking change for anything
  that parses them. This feature lands the text change and flags the breaking surface;
  it does **not** decide the version bump. The `breaking-surface` CI gate applies.

## Tasks

| ID | Specs | Scope | Route | Status | Commit |
|----|-------|-------|-------|--------|--------|
| T1 | S1, S3 | Fase 0 — Governance: `AGENTS.md` (3 sites), `README.md`, `typos.toml`, `.github/ISSUE_TEMPLATE/*.yml` (4), `.github/support.json` `eol_reason` strings | inline | pending | — |
| T2 | S2, S3 | `webfang_core/domain/` — `auth_source.rs` (94), `keyring_store.rs` (51), `local_secret_store.rs` (29), `proxy_config.rs`, `options_spec/`, `error.rs`, `value_objects.rs`, `ssrf_guard.rs`, `http_error.rs`, `body_cap.rs`, plus inline `#[cfg(test)]` assertions | inline | pending | — |
| T3 | S2, S3 | `webfang_core/infrastructure/` — `llm/provider.rs` (48), `wizard/identity.rs` (44), `llm/remote_embedding.rs` (35), `persistence/sqlite.rs` (22), `semantic_cleaner.rs`, `downloader/**`; `obsidian/metadata.rs` stopword list is **not** prose and needs its own decision | inline | pending | — |
| T4 | S2, S3 | `webfang_core/application/` + `webfang_core/cli/` — `cli/args/crawler.rs` (20), `cli/commands/connect.rs` (22), `cli/commands/blocks.rs` (15), `cli/preflight/normalization.rs`, `cli/error.rs`, `crawler/collector.rs` (17) | inline | pending | — |
| T5 | S2, S3 | `webfang_mcp/**` (56 files) — `bin/mcp_server_http.rs`, `mcp_server/handlers/export/**`, `mcp_server/provenance.rs`, unit tests inside `src/` | inline | pending | — |
| T6 | S2, S3 | `webfang_cli/**`, `webfang_test_utils/**` (own prose only), `webfang_benchmark/src/error.rs` (update the exemption comment; its text is already English) | inline | pending | — |
| T7 | S2, S3 | Integration tests + snapshots — 23 files under `crates/*/tests/` (194 Spanish lines) and 34 `.snap` files (29 core, 5 mcp) via `cargo insta review` | inline | pending | — |
| T8 | S2, S3 | `scripts/*.sh` (57 files, ~1070 lines) — remove `# STYLE:` comments and translate error strings, keeping each `test_*.sh` assertion with its script | inline | pending | — |
| T9 | S2, S3 | `docs/**` (14 files) + `SUPPORT.md` (regenerate from `.github/support.json`) + root `REPORTE.md`, `COMPATIBILITY-MATRIX.md` | inline | pending | — |
| T10 | S2, S3, S4 | Final verification: `bash scripts/ci_fast_gate.sh`, full pre-commit gate, `cargo nextest run`, `typos`, coverage report; report the breaking surface to the maintainer | inline | pending | — |

## Log

- **L1 (2026-11-18)** — Owner request, verbatim:
  > Quiero eliminar esta convencion y utilizar unicamente English a partir de ahora.
  > necesito un plan para traducir todos los textos en espanol al English que estan en el repo.

- **L2 (2026-11-18)** — Scope decision. Asked once, with the fork made explicit: product +
  docs, plus `odd/`, plus `evidence/`. Owner selected **"Producto + docs (recomendado)"**:
  `evidence/**` and `odd/**` stay frozen, `CHANGELOG.md` stays untouched in every option.

- **L3 (2026-11-18)** — Inventory evidence, measured with high-precision Spanish markers
  (accented characters plus Spanish function words), before any edit:
  - `crates/*/src/**`: 151 files, 897 lines (353 comment lines, 539 non-comment = literals
    and inline assertions). Per crate: `webfang_core` 794, `webfang_mcp` 90,
    `webfang_cli` 9, `webfang_test_utils` 4.
  - `crates/*/tests/**`: 23 files, 194 lines.
  - `.snap`: 34 files (29 `webfang_core`, 5 `webfang_mcp`).
  - `scripts/*.sh`: 57 files, ~1070 lines (includes the `# STYLE:` lines themselves).
  - `docs/`: 14 files. Root Markdown with Spanish: `README.md`, `SUPPORT.md`,
    `REPORTE.md`, `COMPATIBILITY-MATRIX.md`, `CHANGELOG.md` (excluded by S3).
  - `typos.toml`: 50 allowlist entries.
  - `.github/`: 6 workflows, 4 issue templates, `support.json` (`eol_reason` strings).
  - Total in-scope surface: ~285 files, ~2400 lines of prose.

- **L4 (2026-11-18)** — Constraints discovered while measuring, to be honored:
  - The Spanish rule is not prose-only: 34 insta snapshots and 23 integration test files
    **assert Spanish prefixes**. Message + assertion + snapshot is one atomic unit; a
    partial translation leaves the suite red.
  - `crates/webfang_core/src/infrastructure/obsidian/metadata.rs` holds a Spanish
    **stopword list** (`"el", "la", "los", "las", …`). That is feature data, not UI copy;
    translating it changes `search_obsidian` behavior for Spanish content. It requires a
    product decision, not a translation (T3).
  - `crates/webfang_test_utils/src/lib.rs` contains simulated OS error text in fixtures
    (Windows paths). It is fake foreign data, not a message of ours: do not translate.
  - No CI job enforces Spanish. The real enforcement is the test assertions plus the
    `typos` allowlist, so S1 must remove the allowlist as well as the rule text.
  - `webfang_benchmark/src/error.rs` documents its own exemption from the rule; its text
    is already English and only the comment needs updating (T6).

- **L5 (2026-11-18)** — Environment. Worktree `~/Projects/Rust/webfang-worktrees/feat-english-only-messages`
  (directory name satisfies the identity proxy: `basename` = branch with `/` → `-`).
  `.envrc` written from the tree's own name, 5 lines, `$HOME` literal; `direnv allow`
  applied; `.env` copied from main; `seed_target.sh` → `cold reason=no-seed`
  (normal, not a build); `codegraph init` done; `codedb status` reports `root` = the
  worktree and `head` = `f3b64026b340`. Base commit `f3b64026`; `main` has since
  advanced to `98d4d9e2` (one commit, `test(ai)` #2913), which does not overlap this scope.

- **L6 (2026-11-18)** — Next step: T1 (Fase 0, governance) as the first work unit,
  because every later slice depends on the rule no longer saying the opposite.