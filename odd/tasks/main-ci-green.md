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

- T1 — S1 — route inline — bootstrap worktree + identity manifest — status: done (`.envrc` pending human action: Pi safety guard blocks `.env*` writes)
- T2 — S2 — route inline — `release-plz.toml`: `[[package]] webfang_sandbox_wrapper` + `release = false` — commit: <pending>
- T3 — S2, S5 — route inline — platform-aware expectations in `skip_reason_names_each_missing_prerequisite` — commit: <pending>
- T4 — S2, S4, S5 — route inline — honest degraded LLM artifact (API chapters optional, explicit note) — commit: <pending>
- T5 — S3, S4 — route inline — upstream issue on Dooyo-Labs/rustdoc-markdown — commit: n/a (external record)
- T6 — S1-S5 — route inline — verification (focused test + four cargo gates + fast gate lane), ODD close, Engram mirror — commit: <pending>

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
