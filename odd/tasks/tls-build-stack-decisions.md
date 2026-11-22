# Feature: tls-build-stack-decisions
Branch: docs/tls-build-stack-adr · delivery: one work-unit commit per step · runner: structural doc checks + `cargo check` only if Rust surfaces are touched (none planned).

## Specs

- **S1** — «correr el 47.4 s con el feature set correcto y arreglar el finding #5 rancio». Verbatim from the authorized order: *"Orden analítico que yo seguiría: (1) correr el 47.4 s con el feature set correcto y arreglar el finding #5 rancio, (2) auditar chromiumoxide con esa medición válida, (3) cerrar T5 como doc hygiene, (4) ADR del stack TLS con autorización explícita. CMake queda afuera de todo, como debe ser."* The stale finding #5 states verbatim: *"After T5 the new critical path is `chromiumoxide_cdp` (47.4 s) plus the two C build scripts (btls-sys 41.1 s, libgit2-sys 28.6 s) and 4 binary links (~40 s of unit time)."* Acceptance: the 47.4 s figure is attributed to its actual feature set (`--all-features` vs default), `libgit2-sys` is no longer presented as a post-purge critical-path unit, and the doc preamble no longer claims every timing came from a default-feature `cargo build --workspace`.
- **S2** — «auditar chromiumoxide con esa medición válida». Acceptance: identify where `chromiumoxide_cdp` and the 89 test binaries are built, which CI lanes use `--all-features` vs default features, and whether further feature gating is warranted. **This task is read-only: any Cargo.toml or `.github/` change is out of scope and needs a separate authorization.**
- **S3** — «cerrar T5 como doc hygiene». Acceptance: `build-perf-experiments.md` stops asserting `[profile.dev.build-override] opt-level = 3` as part of the accepted T5 bundle; the checkbox or results table reflects that only `[profile.dev.package."*"] opt-level = 0` landed and was measured. No Cargo.toml profile edit (that would be a new measured decision, not doc hygiene).
- **S4** — «ADR del stack TLS con autorización explícita». Acceptance: a new ADR in `docs/adr/` records BoringSSL/wreq as a deliberate, measured choice, following the house ADR format (Status/Date/Deciders/Related, Context, Decision, Consequences, Alternatives rejected), in English. It must cite ADR-0003 §3, `AGENTS.md`, `build_wreq_client`, the field evidence in `jitter-pacing-2610.md`, and the measured finding that the C build is not the critical path. Explicit authorization for touching `docs/` is the user message in L2.
- **S5** — «CMake queda afuera de todo, como debe ser.» Scope boundary: no build-script, dependency, CI or CMake work in this feature.

## Tasks

- T1 · S1 — route inline — measurement + doc fix — status done — commit bb7fc0cf
- T2 · S2 — route inline — read-only audit — status done — evidence in L4 (no source change)
- T3 · S3 — route inline — doc hygiene — status pending — commit pending
- T4 · S4 — route inline — new ADR — status pending — commit pending
- T5 · S1-S4 — route inline — final verification, mirror, ODD close — status pending — commit pending

Route declaration: all tasks inline. Trigger evidence: one understood deliverable at a time, no independent parallel units, no context backstop. Risk: none — docs and audit only; no high-risk item (no dependency, CI, profile or public API change).

## Log

L1 — vamos a seguir tu orden analitico tienes mi autorizacion

L2 — Authorized order (verbatim from the preceding answer): "Orden analítico que yo seguiría: (1) correr el 47.4 s con el feature set correcto y arreglar el finding #5 rancio, (2) auditar chromiumoxide con esa medición válida, (3) cerrar T5 como doc hygiene, (4) ADR del stack TLS con autorización explícita. CMake queda afuera de todo, como debe ser." This is the explicit `docs/` authorization required by AGENTS.md.

L3 — Baseline verification already completed before this feature: the colleague's verdict was contrasted against the repo; the dependency chain, ADR-0003 §3, `build_wreq_client`, the Cloudflare field evidence and finding #3 were confirmed. Corrections found: T5 `build-override` was never in the results table (only a checked checkbox), `NOTICE` claims `ring 0.17.14` / `rustls-webpki` statically linked though neither is in `Cargo.lock`, `build-perf-experiments.md` finding #5 still lists purged `libgit2-sys`, and the 47.4 s figure has a broken feature-set premise.

L4 — T1 evidence: cold re-measurement in this worktree's virgin target dir with the real cargo (mbx shim bypassed), `cargo build -p webfang_core --features chromium --timings`, rustc 1.91.1, 16 jobs, 540 units, 132.3 s total: `chromiumoxide_cdp v0.7.0` 75.0 s (72.9 s frontend) — the longest unit — and `btls-sys` build script 74.2 s. The 47.4 s and 75.0 s values are not interchangeable (different command/load/revision). Preamble, finding #5 and finding #7 corrected; finding #3/#4 keep libgit2-sys as historical pre-purge measurements. Commit bb7fc0cf.

L5 — T2 audit evidence (read-only): `chromium = ["dep:chromiumoxide"]` with `chromiumoxide` optional and `default = []` in `webfang_core`/`webfang_cli`/`webfang_mcp`; `scripts/test_build_graph.sh` pins "the default workspace graph is browser-free" and `ci.yml` `repo-guards` runs that harness. PR fast gate `test-core` is default-feature; `test-full`/flake nightly use an explicit 6-feature prebuild including chromium; `coverage`/`mcp` are non-gating `--all-features`; no workflow passes `--all-targets` to `nextest`. The 89 binaries / ~270 MB figure is from a local mold experiment, not a CI job. Removing chromium from the prebuild spec would reintroduce private per-test builds and the 180 s parity timeout (`fix-mcp-parity-timeout-2335.md` E2, `cli_harness.rs` `cfg!(feature = "chromium")`, `bin_path.rs` contract). Verdict: no defensible further gating — the 47.4 s problem was a misattributed measurement, not a gating defect. Cargo.toml/.github changes remain outside this feature's authorization.
