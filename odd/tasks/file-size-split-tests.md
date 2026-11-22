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

## Tasks

T0 Worktree + bootstrap + isolation + indexes + this document + issue — done — (this commit)
T1 Evidence: pre-split remote_embedding.rs mod structure (4bc8b9d9), #2950's shipped extraction
(the C5 re-parent diff), F1a/F1b harness patterns (--negation, mutation self-watch), gate
registration pattern (.tsv + ci.yml + test_ci_gate_structure.sh) — pending
T2 split_tests.sh interface + proof semantics; RED harness (test-first) — pending
T3 Implement split_tests.sh; harness green — pending
T4 Acceptance: tool output on real mission data == verified manual extraction (#2950) — pending
T5 Gate registration (.tsv, ci.yml, test_ci_gate_structure.sh green) — pending
T6 Full verification (fast gate, pre-commit gate) + PR-ready state — pending

## Log

L1 (verbatim, 2026-11-22): "si arranca" — the owner approves the mission start and the Ask-first
for the three new files (`scripts/split_tests.sh`, `scripts/test_split_tests.sh`,
`odd/tasks/file-size-split-tests.md`), after the proposal: deliverable (extract `mod tests`
with multiset proof), acceptance (§11 row, validated against #2950's real data), harness
(meta-guard #2056), gate registration, own issue + `Closes part of #2584`. F1a precedent for the
Ask-first record: the explicit approval in chat is the binding record (PR #2926 body, "Ask-first
record").
