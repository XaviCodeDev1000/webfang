# ODD — main-ci-green: three red workflows on main

Objective: restore green `main` by fixing the three independent causes diagnosed on
2026-11-27 (Release-plz, Docs/LLM artifact, cross-platform tests). Branch
`fix/main-ci-green` (worktree `fix-main-ci-green`), base `origin/main` `99471ded`.

## Specs

S1. "puedes decireme que fallos tenemos en main actualmente ? y cuales son y porque?"
    — Diagnosis only, delivered before any change: Release-plz (`cannot find package
    "webfang_sandbox_wrapper"`), Docs (`rustc 1.93.0-nightly is not supported …
    requires rustc 1.100`), CI macOS/Windows
    (`skip_reason_names_each_missing_prerequisite` assertion at
    `downloader_factory/tests.rs:58`).

S2. "ok vamos a arregralos y pero antes de implementar consulta la doc oficial"
    — Official documentation consulted BEFORE implementing. Result: release-plz
    `release = false` excludes a package from `update`/`release-pr`/`release`, and a
    Cargo `publish = false` does NOT exclude it; `rustdoc-markdown` 0.91.0 hardcodes
    `NIGHTLY_RUST_VERSION = "nightly-2025-11-27"` with no CLI/env override and has a
    single published version; `cargo rustdoc --ignore-rust-version` exists but the tool
    owns the invocation.

S3. "i si necesitas abrir issues o insertar label approved tienes permiso para hacerlo"
    — The agent may open the upstream issue and apply `status:approved` to the linked
    repository issue. That grant is the maintainer's approval act, not a self-approval.

S4. Owner decision for the Docs leg (option label, verbatim): "Issue upstream + paso no
    bloqueante" — open an issue on Dooyo-Labs/rustdoc-markdown asking for a nightly
    refresh or a `--toolchain`/`--ignore-rust-version` knob, and meanwhile take the API
    section out of the Docs gate while keeping it visible in the artifact.

S5. Every fix must be doc-backed or reference-backed, never a guess: `release = false`
    (release-plz Configuration, "The release field"), platform-ordered `skip_reason`
    (S3 rule 5 comment + the observed assertion), and an honest degraded artifact that
    never looks complete when the API chapters are missing.

## Tasks

- T1 — S1 — route inline — bootstrap worktree + identity manifest — status: done (owner wrote the 5-line `.envrc`; `reason=own-registered-target`)
- T2 — S2 — route inline — `release-plz.toml`: `[[package]] webfang_sandbox_wrapper` + `release = false` — status: done — commit: 8b59f084
- T3 — S2, S5 — route inline — platform-aware expectations in `skip_reason_names_each_missing_prerequisite` — status: done — commit: f130e919
- T4 — S2, S4, S5 — delegated (context backstop) — honest degraded LLM artifact + stub-renderer harness + docs.yml comments — status: done — commit: 036e15ed
- T5 — S3, S4 — route inline — upstream issue on Dooyo-Labs/rustdoc-markdown — status: done — puddlers/rustdoc-markdown#1
- T6 — S1-S5 — route inline — verification, native review, ODD close, Engram mirror — status: done — commit: <this doc-close commit>

## Log

L1 (verbatim): "puedes decireme que fallos tenemos en main actualmente ? y cuales son y porque?"

L2 (verbatim): "ok vamos a arregralos y pero antes de implementar consulta la doc oficial"

L3 (verbatim): "i si necesitas abrir issues o insertar label approved tienes permiso para hacerlo"

L4 (verbatim, Docs decision): "Issue upstream + paso no bloqueante" — "Abro issue en
Dooyo-Labs/rustdoc-markdown pidiendo una release con nightly ≥ MSRV (o un flag
--toolchain/--ignore-rust-version). Mientras tanto la sección API del artefacto LLM sale
del gate: Docs vuelve a verde, el gap queda trazado en el issue."

L5 — Evidence (2026-11-27, run ids and SHAs):
- Release-plz red since `5e1852f3` (#3051, 2026-11-26T00:20Z), last green `17ef82c7`
  (2026-11-25T21:49Z). Error verbatim: `0: failed to determine next versions` /
  `1: cannot find package "webfang_sandbox_wrapper" in workspace
  "/tmp/release-plz-webfang_core-*/worktree"`. `#3051` is the commit that added
  `crates/webfang_sandbox_wrapper/Cargo.toml` (`git log --diff-filter=A`).
- Docs red since `4158450c` (#3072, 2026-11-27T01:51Z), last green run `ca9bbeeb`.
  Error verbatim: `error: rustc 1.93.0-nightly is not supported by the following
  packages: webfang_core@9.2.4 requires rustc 1.100 / wreq@0.16.0 requires rustc 1.98`.
- CI: `Tests (macos-latest)` fails `skip_reason_names_each_missing_prerequisite`
  (`left: Some("non-linux platform")`, `right: Some("cgroup controllers not
  delegated")`) in runs 46062657471, 46049899268, 46022618760; `Tests
  (windows-latest)` also carries 1 failure of 5710. Introduced by `5e1852f3`.
- Official docs: release-plz Configuration "The release field" + FAQ "Packages with
  `publish = false` in their Cargo.toml are also released (tagged)"; docs.rs
  `rustdoc_markdown::NIGHTLY_RUST_VERSION` (constant) and `run_rustdoc`
  (`rustup_toolchain::install` + `Builder::toolchain`); the CLI (`src/main.rs`) exposes
  no toolchain flag; crates.io lists only 0.91.0; Cargo Book
  `cargo rustdoc --ignore-rust-version`.

L6 — Route: inline; no writer trigger fires (three small units in one concern area, no
  parallel explosion), RDD switch is on so assess runs before any review decision. High
  risk item 5 (delivery/environment: `.github/` and release automation) applies to T4/T2.

L7 — Blocker: the Pi safety guard denies any read/write of `.env*` paths, so the mission
  worktree has no `.envrc`; `scripts/githooks/identity-gate.sh` prints
  `DENY — no CARGO_TARGET_DIR declaration in this tree's .envrc` and the ambient shell
  inherits main's target (`CARGO_TARGET_DIR=~/.cache/cargo-target/main`). Requires the
  human to write the 5-line `.envrc` exactly as obs #2307 §1 prescribes.

L8 — Bootstrap unblocked (owner ran the `.envrc` step): `direnv export bash` yields
  `CARGO_TARGET_DIR=~/.cache/cargo-target/fix-main-ci-green` and
  `scripts/check_target_isolation.sh` reports `target: registered (own)` /
  `reason=own-registered-target`. Seed outcome: `seed: cold  reason=no-seed` (no published
  seed matched this tree's key); `cargo check --all-features` still finished in 1m05s on the
  mbx cache. Non-interactive shells must `eval "$(direnv export bash)"` before anything the
  identity gate reads (the gate DENIES with main's ambient target otherwise — observed).

L9 — T2/T3 evidence: `cargo nextest run -p webfang_core -E
  'test(skip_reason_names_each_missing_prerequisite)'` → 1 passed; `cargo fmt --all --
  --check` clean; `release-plz.toml` parses with the six packages and the wrapper at
  `release: false`. Full gates: `cargo check --all-features`, `cargo clippy --all-targets
  --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`,
  `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` — all green.

L10 — T4 (delegated to a bounded writer) evidence, re-verified by the parent: the harness
  reports `llm artifact degrade: 17 passed, 0 failed` (RED on the pre-change script: 7
  passed, 6 failed, reproducing the exact CI MSRV failure) and the pre-existing
  `scripts/test_artifact_llm_bundle.sh` stays `19 passed, 0 failed`, so the B21 abort
  contract survives. The writer narrowed the degrade to two causes only — an absent
  renderer and cargo's MSRV rejection — because degrading on any failure would have broken
  B21; the parent accepted that narrowing. Parent hardening on review: the skip branch now
  also removes the renderer's `$out.raw`/`$out.tmp` scratch files, since the publish step
  moves the whole staging tree.

L11 — Commits (work units): `8b59f084` release-plz registration + this document,
  `f130e919` cross-platform test expectations, `036e15ed` LLM artifact degrade + harness +
  docs.yml comments. Static guards and the identity gate passed on every commit.

L12 — Native review (RDD on, assess `nativeRisk: high`, `reviewDue: true`): lineage
  `review-8b5fd7c86b9633df`, lenses risk/resilience/reliability (a four-lens START was
  rejected with `lenses-invalid`; three was accepted), 6 files / 680 lines, correction
  budget 200. Outcome **approved**; the exact acknowledgement burned the authority
  (`authority: burned`, `delivery: ordinary-repository-policy`). Two advisory,
  non-blocking findings stand as follow-ups, not corrections: **R3-001** the MSRV
  classification matches cargo's own wording (a future reword turns Docs red rather than
  producing a misleading artifact); **R3-002** the new harness does not exercise the `rc=2`
  abort path through the new classifier.

L13 — T5 record: upstream request filed at
  https://github.com/puddlers/rustdoc-markdown/issues/1 (nightly refresh or a
  `--toolchain`/`--ignore-rust-version` knob). Repository issue with the maintainer's
  `status:approved`: #3107.
