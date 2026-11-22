# ODD — F2: split_tests.sh, the mod-tests extraction with the multiset proof

Objective: phase F2 of the file-size policy (umbrella #2584) — `scripts/split_tests.sh`, the tool
that extracts an inline `mod tests` into a child file with a multiset proof: the extracted mod's
test multiset is identical before/after (nothing lost, nothing invented) and the source's
remaining test multiset is the original minus the extracted. Acceptance (policy §11): "After the
first real mission: the same output as a verified manual extraction" — the first real mission
exists: `remote_embedding.rs` (PR #2950, base 4bc8b9d9), five test mods re-parented by hand and
verified.
Branch: `feat/file-size-split-tests` (worktree `feat-file-size-split-tests`, base main 94de2c51).
Delivery: `ask-on-risk`. Runner: `bash scripts/test_split_tests.sh`.
Ask-first (owner approval recorded 2026-11-22, verbatim "si arranca"): `scripts/split_tests.sh`,
`scripts/test_split_tests.sh`, `odd/tasks/file-size-split-tests.md`.

## Specs

S1 Policy §4.3 step 5 (verbatim): "Execute in slices, one commit per extraction, tests green
between commits, mechanical movement first. split_tests.sh [F2] will extract mod tests with a
multiset proof; until then the verbatim move is proven by hand."

S2 Policy §11 F2 row (verbatim): "| F2 | split_tests.sh (multiset) | After the first real
mission: the same output as a verified manual extraction |"

S3 Policy §8 (verbatim): "Phase labels. Every rule depending on a not-yet-existing tool carries
the [F1], [F2] or [F3] label (the phase delivering it, section 11). While the phase is unmerged,
that step is done by hand with the pasted command and its output; nobody invokes a script that
does not exist. When the phase merges, the label is removed in the same PR." → the F2 PR body
carries the policy-text delta (v3.6.0: [F2] labels retire).

S4 Harness meta-guard #2056 (the harness can fail): `--negation` + mutation self-watch, per the
F1b harness pattern (`scripts/test_file_size_done.sh`). Not a blind guard.

S5 Gate registration (standing norm, policy §11): the new script and its harness register in the
gate in the same PR — the `.tsv` repo-guards row's note names the pair; `ci.yml` repo-guards
wires both; `scripts/test_ci_gate_structure.sh` stays green.

S6 The multiset proof (S1's term, made measurable): extracting a named test mod preserves the
multiset of test items — every test item of the mod appears exactly once in the destination
after extraction, and the source's remaining test multiset equals the original minus the
extracted ones. A dropped, duplicated or renamed test fails the tool (nonzero exit, cause
named).

S7 The extraction's shape reproduces the verified manual extraction of #2950 on real data: the
named mod block is cut from the declaring file, re-parented into the destination (dedented),
the declaring file's block replaced by a declaration — plain `mod <name>;` when the destination
sits in the standard child dir of the declaring file, `#[path = "..."]` otherwise (mission
evidence L4 of `odd/tasks/file-size-remote-embedding.md`).

S8 Governance: own issue + PR with `Closes #<issue>` + `Closes part of #2584`; exactly one
`type:*` label (auto); conventional branch; base `main`; no `CHANGELOG.md`, no `docs/`.

S9 Push/PR/merge remain the owner's decision.

S10 Interface and contract (derived from S1-S7, declared here):
- `scripts/split_tests.sh <source-file> --mod <name> --dest <dest-file> [--header-file <f>]` —
  extracts the named top-level `#[cfg(test)]`-attributed `mod <name> { ... }` block.
- ONE Rust lexer in the program: the preflight's (owner of the inline-tests span since F1a).
  The preflight gains an informational mode `--lex-span <path>` printing one
  `SPAN<TAB><start>:<end><TAB><mod-name>` line per top-level braced mod (`start` = first line
  of the contiguous attribute run above the mod, `end` = the mod's closing brace) or
  `INDETERMINATE<TAB><reason>`; the existing modes and their output are untouched. No second
  lexer in split_tests.sh (one reader, one owner — the preflight's own corpus principle; no
  jscpd clone).
- The multiset proof is an exact partition: `multiset(source_before) == multiset(source_after)
  ⊎ multiset(dest)`, over test items (a `fn` whose contiguous attribute run contains a
  test-running attribute: `#[test]`, `#[tokio::test]`, `#[rstest]`, `#[test_case]`), scanned
  with the same deterministic rule on all three sides. Catches loss AND duplication; a header
  that adds items breaks the partition (the move must be pure).
- Decl shape (S7): keep the block's attribute lines verbatim; plain `mod <name>;` iff dest ==
  `<dirname(source)>/<source-stem>/<name>.rs` (the standard child file), else
  `#[path = "<dest relative to dirname(source)>"] mod <name>;`.
- Emission: preflight-style tab-separated lines, CLOSED English key list (`extracted`,
  `dest`, `decl`, `proof`).
- Exit map (closed): 0 extraction done and the partition holds · 1 tool failure (named: the
  preflight/lexer unreachable, python3 missing, I/O) · 4 defective plan (named: mod not found,
  ambiguous name, target mod not cfg(test)-attributed, INDETERMINATE span, dest == source,
  dest already exists) · 64 usage.
- Gate registration (S5): NO new `.tsv` row (one row per job, #2559) — the repo-guards note
  names the pair; `ci.yml` gains the harness step (and a tool smoke); `ci_fast_gate.sh` gains
  the lane_ci triggered block for the pair. The tool WRITES files: no `run_guard` of the live
  tree in lane_code/full — the harness only.

## Tasks

T0 Worktree + bootstrap + isolation + indexes + this document + issue — done — 88381229
T1 Evidence: pre-split remote_embedding.rs mod structure (4bc8b9d9), #2950's shipped extraction
(the C5 re-parent diff), F1a/F1b harness patterns (--negation, mutation self-watch), gate
registration pattern (.tsv + ci.yml + test_ci_gate_structure.sh) — done — evidence in L2
T2 split_tests.sh interface + proof semantics; RED harness (test-first) — done — RED observed:
22 assertions FAIL with the tool absent (exit 1), 8 self-watch cases green
T3 Implement split_tests.sh + the preflight's --lex-span; harness green — done — F2 harness
60/60, preflight harness 118/118 (cases 20.1-20.11 added), shellcheck clean
T4 Acceptance: tool output on real mission data == verified manual extraction (#2950) — done —
see L3 (inventory 38/38 identical, token-level 38/38 identical)
T5 Gate registration (.tsv, ci.yml, test_ci_gate_structure.sh green) — done — 6641f00b (ci.yml
smoke + harness steps, ci_fast_gate.sh lane_ci triggered block, repo-guards note extended; the
harness's wiring-lock case 8.1-8.3 green)
T6 Full verification (fast gate, pre-commit gate) + PR-ready state — done — see L4; 3 commits,
PR-ready, push/PR the owner's decision (S9)

## Log

L1 (verbatim, 2026-11-22): "si arranca" — the owner approves the mission start and the Ask-first
for the three new files (`scripts/split_tests.sh`, `scripts/test_split_tests.sh`,
`odd/tasks/file-size-split-tests.md`), after the proposal: deliverable (extract `mod tests`
with multiset proof), acceptance (§11 row, validated against #2950's real data), harness
(meta-guard #2056), gate registration, own issue + `Closes part of #2584`. F1a precedent for the
Ask-first record: the explicit approval in chat is the binding record (PR #2926 body, "Ask-first
record").

L2 (2026-11-22): T1 evidence. (a) The pre-split `remote_embedding.rs` (4bc8b9d9) carries ONE
inline `#[cfg(test)] mod tests` (L822-1714, ~893 lines: internal uses + the vtable test at
~831-855 + five thematic sections at 861-1714); #2950 shipped the five thematic children plus a
hub remnant (the vtable test stays inline in the hub's `mod tests`), so the tool's unit is the
whole named mod → one child file, and T4's comparison is the partition identity: the tool's dest
test multiset == the shipped union (five children + the hub remnant). (b) Decl shape from the
shipped tree: seam-declared children use `#[cfg(test)]` + `#[path = "<file>"] mod tests;`
(dest outside the declaring file's standard child dir); hub-declared children use plain
`mod <name>;` (dest IS the standard child file). (c) Harness patterns from
test_file_size_done.sh / test_file_size_preflight.sh: ok/bad/expect_* helpers, hermetic mktemp
sandboxes holding COPIES of the tool (+ the preflight, whose span mode it shells out to),
`--negation` self-invocation that must FAIL on a correct tree (#2056), the family lint (no
pipe into an early-exit `grep -q`, #2925, LINT_PAT `\|[[:space:]]*grep[[:space:]]+(-[A-Za-z]*q[A-Za-z]*|--quiet)`
over comment-stripped lines), the live wiring lock (tsv note + ci.yml + ci_fast_gate.sh name
BOTH scripts), and the closed-English-key emission contract (v3.3 lesson). (d) Registration:
the .tsv gains NO row (#2559, one row per job) — the repo-guards note names the pair;
ci.yml: harness step + tool smoke; ci_fast_gate.sh: lane_ci triggered block (the preflight
harness's shape, L606-635). (e) The preflight's span lexer (lex_span, L207-457) handles
strings, byte strings, raw strings, nested block comments, char literals vs lifetimes, and
reports INDETERMINATE fail-closed; its span predicate collects every top-level braced mod with
its attribute run (mod_re L395), so `--lex-span` re-prints those spans with names — no lexer
is duplicated. (f) The preflight harness's --help assertion checks each known flag by
expect_match (contains), so adding a mode is additive; the mode list prose is stale there
("three modes" naming four flags) — F2 does not own that prose.

L3 (2026-11-22): T3/T4 evidence. Test-first: the harness ran RED first (22 assertions FAIL,
exit 1, tool absent — the family's own self-watch cases green), then the implementation turned
it GREEN: F2 harness 60/60 (exit 0), preflight harness 118/118 (the 20.1-20.11 --lex-span cases
added; the aggregate SPANS format pinned untouched), shellcheck -S warning clean on the three
scripts. Harness defects found and fixed while converging (all in the harness, none in the
tool): an arg-order mismatch between count_name's body and its call sites (the comment's
contract <file> <fn-name> won), a double-0 from `grep -c ... || printf '0'` (grep -c already
prints 0 on no-match), and `grep -E`'s `\s` replaced by `[[:space:]]`. Tool defects found and
fixed: DEST_ABS normalization required the dest's parent to exist (the first extraction of a
hub creates the child dir — now python's abspath + post-proof makedirs). T4 acceptance on the
REAL pre-split remote_embedding.rs (4bc8b9d9, 1714 lines): the tool extracted `mod tests` at
span 822:1714 — 38 test items, partition `before=38 after=0 dest=38` — and the comparison
against what #2950 shipped (gate/wire/retry/adapter/e2e children + the hub's inline vtable
remnant) shows the INVENTORY IDENTICAL (38/38 names, diff empty) and every body TOKEN-IDENTICAL
(38/38, whitespace-collapsed); the only byte-level deltas are rustfmt's re-wrapping from the
mission's fmt commit (c8d040fe), not semantics. The §11 criterion "the same output as a verified
manual extraction" holds. Out-of-seam finding (issue candidate, not fixed here per the two-hats
protocol): the preflight's AGGREGATE --format counts top-level braced PRODUCTION mods into
inline_tests (a fixture with `pub mod prod { }` reported production 1 / inline_tests 15) — F1a's
fixtures never had one; the named SPAN output of --lex-span is unaffected (split_tests.sh
selects by name + cfg(test)). Work-unit note: this slice is one coherent behavior (the tool,
its proof, its span owner and both harnesses) at ~930 authored lines — above the ~400 heuristic;
splitting it would leave an intermediate commit with the contract's proof red or the span owner
uncovered, so the deviation is declared here.

L4 (2026-11-22): T5/T6 evidence. Wiring (6641f00b): ci.yml gains the split-tests --help smoke
and the semantics-harness steps in repo-guards (both scripts named on live lines); ci_fast_gate.sh
gains the lane_ci triggered block for the pair (the TOOL never runs as a live-tree guard — it
writes files); the coverage .tsv's repo-guards note names the pair (no new row, #2559); the
harness's wiring-lock case (8.1-8.3, live lines only) turned GREEN with it. Verification: F2
harness 63/63 (exit 0); preflight harness 118/118; test_ci_gate_structure.sh 27 assertions +
10 self-tests green (run AFTER the wiring edits); the local fast gate GREEN (PASS=24 FAIL=0
SKIP=19, docs+ci lanes) with "file size split-tests semantics (offline)" PASSing in the ci
lane; shellcheck with the CI's exact invocation (`-x --exclude=SC2010,SC2012,SC2027,SC2034,
SC2046,SC2086,SC2126,SC2129` over scripts/*.sh) clean; actionlint clean on ci.yml; the
pre-commit hooks (core.hooksPath=scripts/githooks) ran the full Rust gate on each commit.
git diff --stat main...HEAD: 8 files, +1084/-10 — exactly the mission's files. The PR body
carries the policy-text delta (v3.6.0: the [F2] phase labels retire — policy §8) and
`Closes #2951` + `Closes part of #2584`; #2951 needs `status:approved` before the PR opens.
The PR title (3 commits → queue squash subject = the title): "feat(ci): split_tests.sh, the
mod-tests extraction with the multiset proof (F2)".
