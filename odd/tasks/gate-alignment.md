# Gate alignment — Fase 1

Branch: `chore/gate-alignment` · Issue: #3049 (`status:approved`) · Delivery: one work-unit commit on the feature branch; push/PR remain human decisions.

## Specs

S1. "autorizado y crea la issue con label approved". Issue #3049 was created with `status:approved` under explicit owner authorization.

S2. Align the local automation gates with the canonical CI checks. Verified drift on `main@cd15e330`: `justfile:9` runs `cargo fmt --check` while `ci.yml:1250` runs `cargo fmt --all -- --check`; `justfile:10` uses `-W clippy::pedantic` plus ~30 `-A` allowances while `ci.yml:1279` requires `-D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`; no rustdoc recipe exists while `ci.yml:1930-1932` runs `env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps`; `.pre-commit-config.yaml:26-30` runs `cargo fmt -- --check` (no `--all`); several justfile comments are in Spanish.

S3. Create `odd/gates/canonical-check-spec.md` as the single source of truth for fmt/clippy/rustdoc/language across justfile, pre-commit and CI. CI remains the original canonical source.

S4. Scope: exactly three edited/added automation files (`odd/gates/canonical-check-spec.md`, `justfile`, `.pre-commit-config.yaml`). No changes to `.github/`, `CHANGELOG.md`, `docs/`, `Cargo.toml`, or Rust source. All new or edited comments in English.

## Tasks

T1 | S1-S4 | inline | completed | commit: 5107a05f (Risk: item 5 — shared gate configuration)
T2 | S3 | inline | completed | commit: PENDING

## Log

L1 (verbatim user request): "autorizado y crea la issue con label approved" — authorizes Phase 1 with the three listed files plus the issue.

L2 (verbatim preceding proposal): "¿Autorizas que prepare un worktree y aplique la Fase 1 con estos 3 archivos y las correcciones verificadas?"

L3 (bootstrap, 2026-11-25): worktree `/home/xavi/Projects/Rust/webfang-worktrees/chore-gate-alignment` created from `origin/main@cd15e330`; 5-line `.envrc` written from the tree name and activated; `.env` copied; identity manifest written; CodeGraph (963 files) and CodeDB (1708 files) indexed in the tree; `direnv export bash` reports `CARGO_TARGET_DIR=.../chore-gate-alignment`; both indexes name the worktree and match HEAD; `identity-gate.sh` exits 0.

L4 (delivery): issue #3049 created with `status:approved`, verified label exists and owner instruction applies (not self-approved by the agent).

L5 (verification): `just -n check` and `just -n test-ci` match `odd/gates/canonical-check-spec.md` exactly, in order (check → clippy with both `-W` → `fmt --all -- --check` → rustdoc with `RUSTDOCFLAGS=-D warnings`). YAML parses; `cargo fmt --all -- --check` passes; no Spanish remains in the three touched files. Independent verifier first found an order mismatch (fmt before clippy) and two unaccented Spanish comments; both were fixed and a bounded recheck returned PASS. Full `just check` exited 0; rustdoc emitted ICEs from the installed toolchain (scoped-tls) without failing the gate; generated ICE files were moved outside the worktree to `/tmp/opencode/webfang-ice/`. No `.github/`, `CHANGELOG.md`, `docs/`, `Cargo.toml` or Rust source touched.

L6 (delivery evidence): work unit committed as `5107a05f chore(ci): align local gates with canonical CI checks` (identity gate OK, review-gate reported no governing review authority). RDD assess over `origin/main...5107a05f`: `risk: medium`, `review_due: false`, `review_due_reason: under_budget`; no native review started. Push and PR remain human decisions.

L7 (Phase 2 verification): `scripts/tests/test_canonical_gate_alignment.sh` compares gate command STRINGS/ARGS only — it never runs cargo, just, pre-commit or rustdoc. `bash -n`: exit 0. `shellcheck`: exit 0. Harness: exit 0, 11 passed / 0 failed. GREEN on the unmutated inputs for all five assertions (spec count + order, `just check`, `just test-ci`, pre-commit fmt/clippy hooks, CI fmt/clippy/doc + RUSTDOCFLAGS). RED observed for every NEGATION: drop `--all` → `just check` fails; `-D`→`-A` → pre-commit hook fails; drop one `-W` → `just test-ci` fails; inverted order → `just check` fails; delete `RUSTDOCFLAGS` → CI assertion fails; missing justfile → exit 2 (fail-closed). CI `cargo check` is deliberately not asserted literally — CI uses `-p/--features` variants — so the harness documents it as a local mirror of the spec, not a CI equality claim. No `.github/`, `justfile`, `.pre-commit-config.yaml`, `Cargo.toml` or Rust source was edited in Phase 2. Work unit: PENDING.
