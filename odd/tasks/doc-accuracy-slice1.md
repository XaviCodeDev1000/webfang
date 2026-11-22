# Feature: doc-accuracy-slice1
Branch: docs/false-claims-slice1 · issue: #2995 (umbrella, `status:approved`) · delivery: one work-unit commit per document · runner: `scripts/ci_fast_gate.sh` docs lane + full pre-commit gate.

## Specs

- **S1** — Fix the verifiable false claims in root `NOTICE`: `selectors 0.35.0` (absent from both lockfiles), the release feature set cited as `--features "ai mcp"` (no `ai` feature; release uses `--features webfang_cli/mcp`), `ring 0.17.14` claimed statically linked (`ring` absent from both lockfiles), and `rustls-webpki` / `untrusted` claimed in the runtime graph and in the notices bundle (neither is locked; the bundle is generated from the lockfiles). The self-flagged stale LGPL justification about `wreq-util` in `deny.toml` stays out of scope (D3: needs its own owner decision).
- **S2** — Fix `README.md`: delete `cargo build --release -p webfang_cli --features full` (no `full` feature); replace the copied error model *"DomainError, InfraError → ScraperError (dual wrapping)"* with a pointer to AGENTS.md; remove the `codedb index .` developer-guide line. Owner decision verbatim: *"no hay que incluir codedb en README es una herramienta de desarrollo no forma parte de webfang"*. Also align the weak quick-verification clippy command with the CI command. The `codedb` line is an editorial scope change, NOT a false-claim fix — `codedb index .` was executed and works (exit 0, 1665 files indexed).
- **S3** — Method: capture a RED check before editing (the false strings are present) and the same check GREEN after. No new guard script or `.github/` change in this slice (D2/D4 need separate authorization). No frozen docs touched (`evidence/**`, `CHANGELOG.md`, accepted ADR bodies).
- **S4** — The remaining claim nuclei stay tracked in #2995: `COMPATIBILITY-MATRIX.md` CrawlCheckpoint, three `EmbeddingPort` doc-comments, `deny.toml` LGPL comment, and issue #1817 residual ACs.

## Tasks

- T1 · S1 — route inline — RED check + NOTICE correction — status done — commit 0f60d6d6
- T2 · S2 — route inline — README correction — status done — commit 484463c2
- T3 · S1-S4 — route inline — verification, mirror, ODD close — status done — commit pending

Route declaration: all inline. Trigger evidence: one understood document at a time, no independent parallel units, no context backstop. Risk: none — prose-only; no dependency, CI, profile or public API change.

## Log

L1 — vamos alla

L2 — Owner decision (verbatim): "no hay que incluir codedb en README es una herramienta de desarrollo no forma parte de webfang"

L3 — Authorized proposal (verbatim from the preceding answer): "Cuando queres, abrimos el paraguas y arrancamos con NOTICE + README como primer slice." Umbrella issue #2995 created with `type:docs` + `status:approved` under explicit instruction and verified `admin` permission.

L4 — Census baseline: verified floor of 9 claim nuclei — 5 in `NOTICE`, 1 in `COMPATIBILITY-MATRIX.md`, 3 in `crates/webfang_core/src/domain/embedding_port.rs` doc-comments, 2 in `README.md` (the codedb line was initially miscounted as a third, then retracted after direct execution proved the command works). All five doc guards were run and returned exit 0 while the claims were present — the reason this work is needed.

L5 — Method correction recorded: `codedb --help` does not list `index`, but `codedb index .` exits 0 and indexes 1665 files. Never infer command validity from help output alone; execute it read-only.

L6 — T1 evidence: RED check found 8/8 false strings present (`RED_EXIT=1`). NOTICE §2 now cites `cargo tree -p webfang_cli --features mcp -e normal` matching the release's `--features webfang_cli/mcp` and drops `selectors 0.35.0`; §4 replaces the `ring 0.17.14` claim with the verified statement that no `rustls`/`ring`/`aws-lc` crate is in either lockfile; §5 limits the ISC list to `ego-tree` and `scraper`. The self-flagged LGPL/wreq-util item in §3 remains by design (D3, tracked in #2995). Commit 0f60d6d6.

L7 — T2 evidence: README Developer Guide now uses the CI clippy command, drops `--features full` and the `codedb index .` line (owner decision L2), and replaces the copied error model with a link to AGENTS.md. Commit 484463c2.

L8 — T3 verification: the same 8-string check returns `GREEN_EXIT=0`. RDD `gentle-ai review assess --base-ref 919a70a4 --committed-only` returned `risk: medium` (`executable_change` on NOTICE), `review_due: false`, `review_due_reason: under_budget` — no review ceremony. Six doc guards (`check_docs_claims`, `check_docs_version_pins`, `check_ai_providers_doc`, `check_data_flow_doc`, `check_release_feature_matrix`, `check_agents_doc`) all exit 0. `bash scripts/ci_fast_gate.sh` full lane GREEN (PASS=82 FAIL=0 SKIP=4); coverage names doc-quality partial (cargo doc runs, cargo test --doc does not) and several NOT-covered jobs, none touched by this prose-only change. Full pre-commit gate (check, clippy `--all-targets --all-features` with both lints, fmt `--check`, doc with `-D warnings`) exit 0; rustdoc emitted the known ICE dumps (gitignored), build completed, dumps removed; `git status --short` clean.
