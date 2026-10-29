# #2514 — Drop the `html5ever 0.36.1` copy via the legible/dom_query upstream migration

Objective: implement M1 of issue #2514 — remove the `html5ever 0.36.1` copy from the resolved graph, and track the residual `0.38`/`0.39` pair as accepted structural duplication.
Branch: `chore/legible-05` (worktree `~/Projects/Rust/webfang-worktrees/chore-legible-05`).
Delivery strategy: single PR (#2526) — a dependency bump plus a ratchet update; no slice split is needed.
Test runner: `cargo nextest run --all-features -j 4`, gated by `cargo check` / `cargo clippy --all-targets --all-features` / `cargo fmt --all -- --check`.

## Specs

S1. Verbatim from issue #2514: "**M1 — drop `html5ever 0.36.1` (primary, achievable).** Requires upstream: a `legible` (or `dom_query`) release built against a newer `html5ever`. Then bump `legible` in `[workspace.dependencies]` and re-resolve."
Scope: the resolved graph only — no `[patch]` fork, no pinning hack.
Acceptance: `html5ever 0.36.1` absent from `Cargo.lock`; `cargo tree --all-features -i html5ever@0.36.1` reports no consumers.
Checks: cargo tree probe, lock inspection, CI "Repo guards" and "Code quality" jobs.

S2. Verbatim from issue #2514: "M1: no new duplicate crate names and no new licenses introduced by the `legible`/`dom_query` bump."
Acceptance: the live duplicate-name set equals the committed `scripts/duplicate-crate-baseline.txt` names — no new name, no stale line; no new license entry in the lock.
Checks: `scripts/check_advisory_policy.sh --print-duplicates` diffed against the baseline; CI "Code quality (machete + duplication ratchet)".

S3. Verbatim from issue #2514: "M2 (if reachable): single `html5ever` version in the tree, ratcheted down in `scripts/duplicate-crate-baseline.txt`; otherwise documented as accepted structural duplication."
Acceptance: M2 is upstream-blocked — `htmd 0.5.6` (latest) still requires `html5ever ^0.38` — so the residual `0.38`/`0.39` pair is documented as accepted structural duplication in the baseline's dated entry, not silently ignored.
Checks: upstream `htmd` manifest inspection; baseline entry content.

S4. Derived from the issue's "Out of scope" rule ("`[patch]` forks or pinning hacks to force unification — the manifest requirements decide; upstream movement is the clean path.") and the repo rule that an output change nobody asked for is a regression: the bump must not change observable extraction output.
Acceptance: the chosen `legible` release behaves byte-identically to `legible 0.4.2` on `title`, `excerpt` and `text_content` for both the selector fixture and the full page; the full suite passes with zero snapshot changes.
Checks: scratch probe comparing 0.4.2 / 0.4.3 / 0.5.1; `cargo nextest run --all-features`; `git diff` showing no `.snap` change.

S5. Verbatim from the repo rules: "Do not touch `CHANGELOG.md` in a work PR — there is a single writer (the Release PR)." and "Do not touch `docs/` unless explicitly asked."
Acceptance: neither `CHANGELOG.md` nor `docs/` appears in the PR diff.
Checks: `git diff origin/main...HEAD --name-only`.

## Tasks

- T1 (S1, S3) — inline — verify the upstream release train: `legible 0.4.3` (via `dom_query 0.28` → `html5ever ^0.39`) and the behavior-changing `legible 0.5.1`. Evidence: crates.io dependency API + scratch probe. Status: done.
- T2 (S1, S4) — inline — re-resolve the lock onto `legible 0.4.3`; the manifest requirement stays `legible = "0.4"` because it already admits 0.4.3. Commit `38c8d374`. Status: done.
- T3 (S2, S3) — inline — update `scripts/duplicate-crate-baseline.txt`: drop the converged `tendril` line, refresh the `selectors` doc line, add the dated #2514 entry. Commit `38c8d374`. Status: done.
- T4 (S4) — inline — local gates: `cargo check --workspace --all-features`, `cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines`, `cargo fmt --all -- --check`, full suite (5097 tests run, 5097 passed, 9 skipped). Status: done.
- T5 (S4, S5) — inline — PR #2526 body, issue comment correction, CI green, native assessment. Status: in progress (CI running on head `0a2449e4`).

## Log

L1 (2026-10-29, reconstructed from the session's compacted summary — the runtime did not retain the original prompt verbatim): "Reviewar la issue webfang #2514 (dedup de `html5ever`) y ejecutar M1: bumpear `legible` para sacar `html5ever 0.36.1` del grafo. Entregar: comentario en la issue (A) y PR con el bump (B), investigando con Context7/las fuentes upstream el cambio de comportamiento de `legible 0.5.1` que rompió un snapshot."

L2 (2026-10-29): Context7 has no entry for the `legible` crate (it returned Shield.io typography material), so the investigation moved to primary sources — the crates.io dependency API, docs.rs, the `.crate` tarballs, and `ryanfowler/legible` on GitHub (no releases, no CHANGELOG). The upstream truth that mattered: `legible 0.5` rewrote its DOM in-tree (`src/dom/*`, new `src/markdown.rs`, `src/text.rs`, `src/selectors.rs` deleted) and `legible 0.4.3` merely bumped `dom_query` to 0.28, which already requires `html5ever ^0.39` / `selectors ^0.38` / `tendril ^0.5`.

L3 (2026-10-29): first implementation used `legible 0.5.1` (commit `5a8b11bb`) — graph converged to two `html5ever` copies, but CI failed in two test jobs on one snapshot: `behavioral__selector_h3_extracts_only_h3`. The diff: 0.5.1 populates the article excerpt where 0.4.2 returned `None`, so the frontmatter gained an `excerpt:` line. The issue never asked for an output change → treated as a regression, not accepted.

L4 (2026-10-29): scratch probe (`/tmp/opencode/probe-042`, `/tmp/opencode/probe-043`, separate crates because Cargo forbids two versions of one crate in one binary) on the selector fixture and the full page: 0.4.2 and 0.4.3 are byte-identical in `title`, `excerpt` and `text_content`; 0.5.1 differs (`excerpt`). Conclusion: 0.4.3 reaches the same graph state as 0.5.1 through `dom_query 0.28` with zero behavior change.

L5 (2026-10-29): pivot committed as `38c8d374` ("chore(infra): reach html5ever 0.39 through legible 0.4.3, not the 0.5 rewrite"). The 0.5.1 commit stays in history (pushed history is immutable) and is corrected on top.

L6 (2026-10-29): `origin/main` advanced twice during the work (`7f096ee8`, then `d81136cf`); both were merged in so the branch stays current and the native candidate is not polluted by a stale base. Net PR diff against main: `Cargo.lock` + `scripts/duplicate-crate-baseline.txt` (2 files).

L7 (2026-10-30): baseline re-measured on the pivoted tree — 44 baseline names = 44 live names, zero stale, zero new duplicates; `tendril` stays converged on 0.5.1. The `behavioral__selector_h3_extracts_only_h3.snap.new` produced by the 0.5.1 experiment was deleted, so the PR carries no snapshot change.
