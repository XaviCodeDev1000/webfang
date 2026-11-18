# English-only user-facing messages

Remove the Spanish-language convention for user-facing messages and translate the
in-scope Spanish text of the repository to English. Branch `feat/english-only-messages`.
Delivery: `ask-on-risk` (breaking-surface decision belongs to the maintainer).
Test runner: `cargo nextest run` + `cargo insta review` + `bash scripts/ci_fast_gate.sh`.

**Sequencing (owner decision, L7): easiest first, runtime-risk last.** Fase A carries no
runtime risk (rule text, comments, prose). Fase B touches tooling and scripts. Fase C is the
breaking surface (CLI and domain error strings) and is deferred to a later session.

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
| A1 | S1, S3 | Fase A — Governance text: `AGENTS.md` (3 sites: § Project rules, § Error stratification, pre-PR checklist) + `README.md` § Architecture rules | inline | done | `2e8befd9` |
| A1b | S1, S3 | Fase A — Governance hardening: broaden the `AGENTS.md` rule to every repository-facing artifact, and remove the competing "Spanish message" rule in `docs/error-classification-matrix.md` with an amendment note | inline | done | `8347f463` |
| A2 | S2, S3 | Fase A — Comments and doc-comments in 25 files under `crates/**/src/**`; literals and assertions untouched and still deferred to Fase C | inline | done | `693f49c1` |
| A3a | S2, S3 | Fase A — `docs/src/SUMMARY.md` chapter title | inline | done | `c3062f7e` |
| A3b | S2, S3 | Fase A — pure-prose docs: `architecture/ai-providers-design.md` (149), `adr/0004-onnx-local-removal.md` (106), `research/ci-test-playbook.md` (42), `security/prompt-injection-policy.md` (37), `adr/README.md` (33), `llm-backend-decision.md` (29), `research/missing-docs-audit.md` (20), `src/ai-onnx-migration.md` (18) | inline | done | `03c7c5fa` |
| A3c | S2, S3 | Fase C-coupled docs that quote CLI output verbatim: `src/troubleshooting.md`, `ssrf-layers.md`, `src/cli-reference.md`, `src/state-recovery-runbook.md`, `adr/0018-…`, root `REPORTE.md`, `COMPATIBILITY-MATRIX.md` | inline | pending | — |
| B1 | S1, S3 | Fase B — `.github/` complete: ISSUE_TEMPLATE (4), `support.json` `eol_reason` + `render-support-md.sh` + regenerated `SUPPORT.md`, and 7 workflows (`ci-health`, `pr-validation`, `ci`, `release`, `cut-patch-tag`, `docs`, `bug-discovery`) | inline | done | `cb0d177e`, `6b2f9a52`, `7d8f0218` |
| B2 | S2, S3 | Fase B — `scripts/*.sh`: `# STYLE:` comments + error strings, each `test_*.sh` assertion moving with its script. Done: 14 comment-only scripts, the `check_no_ai_attribution` pair, the `check_support_drift` pair, the `rotate-stable` pair. Remaining ~1543 lines; largest are the claims-gate pair `check_ai_providers_doc.sh` (140) + `test_ai_providers_doc.sh` (284) | inline | in_progress | `3b20140d`, `05a44ca0`, `bc6c3bbd`, `e3a48a4d` |
| B3 | S1, S3 | Fase B — `typos.toml`: prune the Spanish allowlist (50 entries). Must run LAST: pruning before the Spanish text it allowlists is gone makes `typos` fail | inline | pending | — |
| C1 | S2, S3 | Fase C — `webfang_core/domain/` error strings (highest breaking risk) | inline | pending | — |
| C2 | S2, S3 | Fase C — `webfang_core/cli/` + `application/` error strings | inline | pending | — |
| C3 | S2, S3 | Fase C — `webfang_mcp/**` strings | inline | pending | — |
| C4 | S2, S3 | Fase C — 23 integration test files + 34 `.snap` via `cargo insta review` | inline | pending | — |
| C5 | S2, S3 | Fase C — `obsidian/metadata.rs` stopword list: product decision, not a translation | inline | blocked | — |
| C6 | S2, S3, S4 | Fase C — final verification + breaking-surface report to the maintainer | inline | pending | — |

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
    product decision, not a translation (C5).
  - `crates/webfang_test_utils/src/lib.rs` contains simulated OS error text in fixtures
    (Windows paths). It is fake foreign data, not a message of ours: do not translate.
  - No CI job enforces Spanish. The real enforcement is the test assertions plus the
    `typos` allowlist, so S1 must remove the allowlist as well as the rule text.
  - `webfang_benchmark/src/error.rs` documents its own exemption from the rule; its text
    is already English and only the comment needs updating (B2/A2).

- **L5 (2026-11-18)** — Environment. Worktree `~/Projects/Rust/webfang-worktrees/feat-english-only-messages`
  (directory name satisfies the identity proxy: `basename` = branch with `/` → `-`).
  `.envrc` written from the tree's own name, 5 lines, `$HOME` literal; `direnv allow`
  applied; `.env` copied from main; `seed_target.sh` → `cold reason=no-seed`
  (normal, not a build); `codegraph init` done; `codedb status` reports `root` = the
  worktree and `head` = `f3b64026b340`. Base commit `f3b64026`; `main` has since
  advanced to `98d4d9e2` (one commit, `test(ai)` #2913), which does not overlap this scope.

- **L6 (2026-11-18)** — First sequence drafted as T1–T10 by layer.

- **L7 (2026-11-18)** — Owner re-sequenced by risk, verbatim:
  > empezamos con lo mas facil de hacer, dejamos lo complicado por ejemplo "los strings de
  > error del CLI" para el final o otra sesion

  The T1–T10 list was rewritten as A/B/C: Fase A (rule text, comments, prose) has no
  runtime risk; Fase B (scripts, `.github`, `typos`) touches tooling; Fase C (CLI and
  domain error strings, tests, snapshots) is the breaking surface and moves to the end or
  to another session. `typos.toml` (old T1) moved to B3 because pruning the allowlist
  before the Spanish text is gone would break the `typos` check.

- **L8 (2026-11-18)** — A1 executed: `AGENTS.md` lines 84, 168 and 561 and `README.md`
  line 511 now state English-only. No code, no test and no snapshot touched, so no build
  or suite run applies to this unit; the change is prose-only.

- **L9 (2026-11-18)** — A2 executed (`693f49c1`, 25 files). The translation was safe, but
  the classification behind it was wrong, and the record is corrected:
  - The high-precision detector (accented characters plus Spanish function words) missed
    Spanish text that carries none of its markers: `produccion`, `paso`, `Clonable`,
    `patrón:`, `desafío`, `Límite`, `inválida`. `typos` surfaced two of them, and only
    because the files became changed files.
  - Consequence: several files classified as pure-comment also hold Spanish **literals** —
    `crawler_config.rs` (`"URL de sitemap inválida …"`), `semantic_cleaner.rs`
    (`"el endpoint de embeddings devolvió el estado 413"`), `tests_context.rs` (`"patrón:"`),
    `tests_inspect.rs` (`"desafío"`), `tests_session_cap.rs` (`"Límite"`). Those are error
    and message surface, so they stay deferred to Fase C per L7. The comment commits do not
    make those files clean, and A2 must not be read as "comments done repo-wide".
  - `tests_panic.rs` and `provenance.rs` legitimately mention Spanish characters (`ñ`, `é`)
    because they test unicode handling; that prose stays.
  - Verification for A2 was proportionate to a comment-only change: `cargo fmt --all --check`
    (parses every file) and `typos` on the changed files. No build or suite was run; the
    target dir was cold (`seed: cold reason=no-seed`) and a BoringSSL build is not justified
    by comments and one `deprecated(note)` string. The `deprecated(note)` string is asserted
    nowhere (checked by search).
  - Lesson for Fase C: the detector is not a scoping tool. Scope by `typos` plus an
    accented-character scan plus a full read of each file, and treat message + assertion +
    snapshot as one unit.

- **L12 (2026-11-18)** — A3b executed (`03c7c5fa`, 8 files, +932/−950). Findings:
  - `docs/architecture/ai-providers-design.md` carries a **claim manifest**
    (`<!-- claims:begin -->` … `<!-- claims:end -->`) verified by
    `scripts/check_ai_providers_doc.sh`. Measured before editing: `claim` and `shape` read the
    cited **source** line, not the doc; only `impl` compares the doc fence against the source,
    and it strips comments first. So prose and comment translation is safe, but the manifest's
    literal source strings must stay verbatim. Gate re-run after the change: `OK … 30 claim,
    8 shape, 5 impl, 1 absent; 47 cita(s) de prosa cubiertas; 26 referencia(s) § resueltas`.
  - The manifest still contains two Spanish literals — `Credencial en variable de entorno
    (legacy, solo CI o desarrollo)` at `auth_source.rs:72` and ``Sin variante `Inline` en v1``
    at `auth_source.rs:6` — because they assert the **current source text**. They become
    translatable only when Fase C translates those source lines, in the same change.
  - `docs/adr/0004-onnx-local-removal.md` keeps one Spanish string on purpose: line 83 quotes
    the real PR title `chore: canal de objeción — migración desde ONNX local` (#1997). It is a
    historical citation that must stay findable with `git log --grep`; translating it would
    break the reference.
  - The original `adr/0004` contained a corrupted fragment (`tres de esos ficheros都属于 al
    crate`) — Chinese characters inside Spanish prose. Fixed in passing.
  - `typos` on the original `ci-test-playbook.md` reported 15 errors; after the translation it
    reports 2 (`label-messager`, `SME`), both legitimate proper nouns. `typos` is not wired
    into any `.github/workflows/*.yml`, so it is not a CI gate here.
  - No Markdown build was run; the claims gate is the real check for this batch.

- **L13 (2026-11-18)** — Fase B started, owner-authorized with the constraint *"no hagas
  cambios a ciegas"*. Method applied: recon first (classify comment-only vs mixed, map every
  cross-file assertion), then change each coupled pair together and run its own test.
  - `3b20140d` — 14 comment-only scripts. Most hits were `ADR-0004 paso 6` (Spanish for "step 6") and `ítem`.
  - `05a44ca0` — `check_no_ai_attribution.sh` + `test_no_ai_attribution.sh`; suite: 33 passed, 0 failed.
  - `cb0d177e` — `.github` issue templates + `support.json` `eol_reason` + `render-support-md.sh` +
    regenerated `SUPPORT.md` (`--check` green). Verified `eol_reason` is **not** rendered into
    `SUPPORT.md`, so the two halves are independent.
  - `bc6c3bbd` — `check_support_drift.sh` + `test_support_drift.sh`. The test carries 13 exact
    `grep -qF` assertions coupled to gate strings; all were updated in the same change.
    Suite: 69 ok, 0 fail.
  - `e3a48a4d` — `rotate-stable.sh` + `test_rotate_stable.sh`, plus the last coupled grep
    (`test_support_drift.sh:716`, which asserts `rotate-stable.sh`'s output). Suites: 25 ok / 0
    fail and 69 ok / 0 fail.
  - `6b2f9a52` — 6 workflows (`docs`, `cut-patch-tag`, `bug-discovery`, `release`, `pr-validation`,
    `ci`). All re-parsed with `yaml.safe_load`.
  - `7d8f0218` — `ci-health.yml` (615 lines, dormant). Verified that **only comments** changed:
    the 3 lines with trailing comments keep their code byte-identical.
  - Detector lesson, third occurrence: the high-precision pattern missed `argumento inesperado`,
    `no existe`, `solo ramas`, `versión X.Y.Z` and `tag ya existe`; they were found by reading the
    files. The detector is a triage aid, never a license to bulk-edit.
  - Remaining in B: ~1543 lines across the script harnesses. B3 stays last by construction.

- **L14 (2026-11-18)** — Claims-gate pair, the most delicate slice of B: it is wired in
  `ci.yml:755` and in the fast gate, so a silent break would red the required `CI Gate`.
  - `943f14e8` — the gate's ~40 findings messages plus the 7 exact `grep -q` patterns the
    test asserts on them. Test: **49 ok, 0 fail**.
  - `878e31a5` — the gate's remaining 77 comment lines. Test re-run: **49 ok, 0 fail**.
  - `scripts/check_ai_providers_doc.sh` is now at **0 Spanish lines**.
  - `scripts/test_ai_providers_doc.sh` keeps **277 Spanish lines**, all of them test
    labels/echos (`pass`/`fail`/`echo "== … =="`), not assertions. They are
    developer-facing output and move in their own slice.
  - Coupling map, for the record — the test asserts exactly these gate strings: `C1: the
    cited line is outside the file`, `C4: the line does not contain the declaration`,
    `escapes the repo root`, `C4: the fence line`, `C1: the manifest block does not exist`,
    `C1: the manifest block does not contain a fence with info-string 'claims'`,
    `C1: the manifest is empty`.

- **L10 (2026-11-18)** — A3 split after reading the small files. Five of the six smallest
  docs (`src/troubleshooting.md`, `ssrf-layers.md`, `src/cli-reference.md`,
  `src/state-recovery-runbook.md`, `adr/0018-…`) are **verbatim quotes of CLI output**
  (`"SSRF detectado: la IP … está prohibida …"`, `"URL inválida: {0}"`, `--help` text). They
  are documentation of the strings, so they must move with the code in Fase C, not before:
  translating them now would document output that does not exist yet. Only
  `src/SUMMARY.md` was pure prose, and it is done. The remaining A3b prose block is ~534
  lines across 8 files; the largest are `architecture/ai-providers-design.md` (149) and
  `adr/0004-onnx-local-removal.md` (106).

- **L11 (2026-11-18)** — A1b, governance hardening. The owner's point: `AGENTS.md` is the
  cause of every agent's behavior, so a narrow rule is not enough. Two gaps were closed:
  - The A1 wording ("user-facing messages, errors, logs and tracing fields") did not cover
    comments, docs or commit messages, so an agent could keep writing Spanish comments
    without breaking it. The rule now covers **every repository-facing artifact** and states
    that the Spanish strings still in the tree are a migration in progress, not a precedent.
  - `AGENTS.md` delegates error semantics to `docs/error-classification-matrix.md` ("Full
    matrix and exit-code rules"). That document is a **closed contract** and still said
    "Spanish message on stderr" (twice, in the MCP projection table). It was the competing
    normative source that would have kept agents emitting Spanish errors. Both cells now read
    "message"/"text", and an amendment note was added under the Status line: no exit code,
    class or mapping changed.
  - Verified: no other agent-instruction surface mentions Spanish (`.kilo/`, `.github/`,
    `.agents/skills/`, `README.md`, root Markdown).