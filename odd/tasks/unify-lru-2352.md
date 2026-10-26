# unify-lru-2352

Objective: reach a single `lru` version in the lock and clear RUSTSEC-2026-0253 (issue #2352).
Branch: `chore/unify-lru-2352` · Worktree: `~/Projects/Rust/webfang-worktrees/chore-unify-lru-2352` · Delivery: local branch, no PR (owner instruction).

## Status

Slice 1 (done): the patched half. Slice 2 (blocked on a maintainer decision): the single-version half.

## Specs

- **S1** — `lru 0.18.1 -> 0.18.5`. The advisory is patched from `>= 0.18.2`; `wreq` requires `^0.18.0`, so this is a lock-only change with no API movement.
- **S2** — one `lru` in `Cargo.lock`. **Not achieved.** See the fork below.
- **S3** — `cargo deny check` and `scripts/check_advisory_policy.sh` green, with no new duplicate crate and no new license.

## The fork

`lru` is never a direct dependency: `html-to-markdown-rs 2.20` requires `lru ^0.16`, `wreq` requires `^0.18`. The 0.16 line has no patched release, so `lru 0.16.4` can only leave the lock if `html-to-markdown-rs` moves to 3.x — the first 3.x without `lru` is 3.11.0.

Measured cost of that bump (3.18.6, resolved against this lock):

- removes: `lru 0.16.4`, `markup5ever_rcdom`, `xml5ever`
- adds **8 new duplicate names**: `phf`, `phf_codegen`, `phf_generator`, `phf_macros`, `phf_shared` (a second phf family, 0.14 next to cssparser's 0.13), `string_cache`, `string_cache_codegen`, `web_atoms` (html5ever 0.40, a fourth html5ever in the tree)
- adds a new license: `tiny-keccak 2.0.2` (CC0-1.0) via `ahash -> const-random -> const-random-macro`, not in `[licenses].allow`
- every 3.x pays some of this: 3.11-3.13 bring the `phf` family instead of the html5ever chain

Net: the "unify a duplicate" chore would add five duplicate names. That is the decision left open, not an oversight.

Also wrong in the issue text: RUSTSEC-2026-0253 **is** already in `deny.toml` with `# Expires: 2027-03-20` and a written rationale, so "el purgado fue RUSTSEC-2026-0002" describes an older state of the file.

## Tasks

- **T1** — `cargo update -p lru@0.18.1` -> 0.18.5. **done**.
- **T2** — `cargo update -p html-to-markdown-rs` to 3.x and port `convert_to_markdown` to the new `ConversionResult` return type. **done and reverted** — see the fork.
- **T3** — Decision on the fork. **open**.

## Log

- **L1** — Owner instruction, verbatim: «vamos a implementar las issues abiertas 2351/2352/2353/ cada una en su propio worktree y no abrimos pr's».
- **E1** — `cargo audit` before: 9 allowed warnings, two of them `lru` (0.16.4 and 0.18.1). After slice 1: 8 warnings, one `lru` (0.16.4).
- **E2** — Advisory text: `patched = [">= 0.18.2"]`; trigger needs a panicking `Drop` inside `LruCache::pop()`.
- **E3** — `cargo tree -i lru@0.16.4` -> html-to-markdown-rs; `cargo tree -i lru@0.18.1` -> wreq (and wreq-util).
- **E4** — 3.x also pulls `rmcp`/`axum`/`tower-http`/`comrak`, but behind the `mcp`/`mcp-http` features, which stay off: none of them reached the lock.
- **E5** — The 3.x attempt compiled and the converter port was green (`convert` now returns `ConversionResult`, `content: Option<String>`); 68 converter/markdown/file_saver tests passed. Only the dependency-policy cost blocked it.
- **E6** — Slice 1 gates: `cargo deny check` all ok; `check_advisory_policy.sh` 42 live duplicates all in baseline, 8 justified ignores, no new duplicate.
