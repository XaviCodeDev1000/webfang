# Mission: file-size-done (F1b)

**Objective**: phase F1b of the file-size policy (umbrella #2584) — `scripts/file_size_done.sh`,
the close-out tool that decides when a split mission is DONE. The exit map is closed
(0 closed / 1 defect / 2 parked, usage 64); `exceptions` and `hard_limit` are read from the
base commit (`git show <base>:scripts/quality-baselines.json`) and the current one, so a
mission cannot grant itself an exception or move the limit.
**Branch**: `feat/file-size-done` in `~/Projects/Rust/webfang-worktrees/feat-file-size-done/`,
based on `origin/main` (98a26c39 at open).
**Delivery**: ONE PR, work-unit commits; runner: `bash scripts/test_file_size_done.sh`
(plus `bash scripts/test_ci_gate_structure.sh` for the registration). The PR body carries the
policy-text delta (v3.5.0: the [F1b] phase labels retire — policy §8).

## Specs

S1 — the tool and its closed exit map, quoted verbatim from the policy (§11 F1b, the body of
PR #2926, v3.4.0):

> | F1b | file_size_done.sh. Exit 0: parts ≤ hard_limit, or the file already in exceptions at
> the base commit. Exit 2 (parked): still over the limit and not in it. Exit 1 (defect): the
> diff adds or empties an exceptions entry, adds an allow, removes a grandfathered entry with
> the file over the limit, or changes hard_limit | A harness with one green close, one parked
> (wreq_downloader, 764) and one defect per exit-1 discriminator. The .tsv registration and
> meta-guard #2056 without marking it a blind guard

S2 — the close step it mechanizes (§4.6, quoted):

> 6. Close. file_size_done.sh <path> [F1b]: exit 0 = closed (parts ≤ hard_limit, or the file
> has an exceptions entry already present in the mission's base commit, --remove done,
> functions under the threshold, no new allow in the diff, jscpd cx not raised, golden
> refreshed, the same test set per the declared runner's <runner> list; --list is not a guard
> mode); exit 2 = parked with a reason; exit 1 = a defect with its cause: the mission's diff
> adds an entry to exceptions (or empties it), adds an allow, removes a grandfathered entry
> with the file still over the limit, or changes hard_limit. exceptions and hard_limit are
> read from the base commit (git show <base>:scripts/quality-baselines.json) and the current
> one.

Scope note on S2 (declared in the PR, not invented): the F1b deliverable is the exit map of
S1 plus the pieces done can measure offline. `functions under the threshold` is F3 (the
baseline's reserved `functions` key is its home). `jscpd cx` is already gated in CI
(check_duplication.sh) and `golden refreshed` by the guard's own repo-guards step; done
delegates to them and says so in --help. `the same test set per the declared runner's <runner>
list` is the mission's own evidence (F4's record template), not a measurable of this tool.

S3 — the parked record (§7, quoted):

> parked: reason, date and base commit.
> Partial reduction. A file that shrank but stays over 500 is parked, never done or exception.
> parked is not new vocabulary: it is the guard's warning branch ... plus two mandatory
> effects: --update path=<n> and a line in the mission with reason, next step and base commit.
> This is exercised today by the guard; file_size_done.sh will return it as exit 2 [F1b].

S4 — the harness acceptance (§11 F1b, quoted in S1): one green close, one parked
(wreq_downloader), one defect per exit-1 discriminator; the meta-guard #2056 property (the
harness can fail: --negation + mutation self-watch), never a blind guard.

S5 — gate registration (the standing norm, policy §11: "Every new script registers in the gate
in the same PR"): the .tsv repo-guards row's note names both scripts; ci.yml repo-guards wires
both (the tool via its informational `--list` smoke — "--list is not a guard mode" (§4.6); the
harness as a semantics-harness step); ci_fast_gate.sh triggers the harness on the pair's diff
(the F1a pattern). #2056: "a guard reachable from no workflow is a guard that does not exist".

S6 — the exit map is documented and tested (v3.4.0 changelog, quoted):

> done keeps 0/1/2, and `--help` documents the map (the per-tool-map proposal of mission round
> 5: each code documented and tested — harness 3.3/3.4)

Usage errors exit 64 (EX_USAGE), the preflight's own v3.4.0 decision (issue #2928); tool and
environment failures fail closed with a named cause at 1 — the message line carries the
`defect:`/`error:` distinction so a human tells a governance defect from a broken environment.

S7 — policy-text delta (§8, quoted): "Every rule depending on a not-yet-existing tool carries
the [F1], [F2] or [F3] label (the phase delivering it, section 11). While the phase is
unmerged, that step is done by hand with the pasted command and its output; nobody invokes a
script that does not exist. When the phase merges, the label is removed in the same PR." The
PR body carries the amended policy text (v3.5.0): the [F1b] labels retire from §4.6 and §7 and
the §11 F1b row is marked Done with this mission's evidence.

S8 — Ask-first record (AGENTS.md "Ask first"; the F1a precedent). The owner approved the new-
file group explicitly (2026-11-21, chat): asked —

> Apruebo agregar estos 3 archivos nuevos fuera de crates/ (regla "Ask first" de AGENTS.md)
> para F1b: scripts/file_size_done.sh, scripts/test_file_size_done.sh,
> odd/tasks/file-size-done.md. ¿Apruebas el grupo?

answered —

> "Sí a los 3 (Recommended)"

S9 — engineering rules carried from F1a's record: English-only output (AGENTS.md); hermetic
harness (mktemp sandboxes, the tool exercised as a COPY, nothing written to a measured tree);
`grep -E ... > /dev/null` never `grep -q` under pipefail (issue #2925); emitted keys asserted
against a CLOSED ENGLISH LIST fed by real outputs of every branch (v3.3 lesson); the corpus is
extracted from check_file_size.sh at runtime — one corpus, one owner (three-way lock with
AGENTS.md stays the preflight's own check 2.2; done re-uses the extraction, never re-implements
it); numbers are measured, never written (the parked fixture asserts the emitted count equals
the file's `wc -l`, not a constant).

S10 — --list is an informational dashboard over the baseline's grandfathered entries (per-file
state and verdict, exceptions annotated), exit 0, labeled "not a guard mode" (§4.6's
parenthetical); it never gates.

## Tasks

- T0 (S8, S1–S10): this record. ✅ (first commit)
- T1 (S4, S6, S9): harness RED — `scripts/test_file_size_done.sh`, hermetic git-fixture
  sandboxes; RED confirmed against the missing tool before T2.
- T2 (S1, S2, S3, S6, S9, S10): tool GREEN — `scripts/file_size_done.sh`.
- T3 (S5): registration — .tsv note, ci.yml steps, ci_fast_gate.sh trigger; test_ci_gate_structure.sh green.
- T4 (S4, S9): full verification pass (harness + --negation, gate structure, fast-gate lanes, pre-commit gate).
- T5 (S7): policy-text delta drafted for the PR body (v3.5.0).

## Log

- L1 (2026-11-21): "Implementar F1b — file_size_done.sh. Es el próximo deliverable que la
  propia política ya define y desbloqueó: mapa de exit codes cerrado (0/2/1), contrato de
  harness especificado en el body de #2926, decisiones tomadas. Es además la herramienta con
  la que se cierra cada split futuro — conviene tenerla antes de la Ola 2." — the user's
  original request, verbatim.
- L2 (2026-11-21): Ask-first approved ("Sí a los 3", S8). Worktree `feat-file-size-done`
  bootstrapped (5-line .envrc from the tree's own name, identity manifest, codegraph + codedb
  indexed at 98a26c39; isolation checks observed: direnv exports the tree's CARGO_TARGET_DIR,
  both indexes root+head match, identity gate exit 0).
- L3 (2026-11-21): contract reading locked (evidence: the #2926 body, v3.4.0): (a) "adds or
  empties an exceptions entry" is implemented as ANY change to the exceptions map at key
  granularity (added / removed / value-changed) — §5 declares exception changes a
  maintainer decision in their own PR that "never happens inside that mission", so a mission
  diff touching the map is the defect class the row names; (b) the allow check is a MULTISET
  over the named files (target + parts) at base vs HEAD: a pre-existing allow passes (F3's
  fetch_inner criterion), a moved allow passes, a net addition is the defect — detection is
  line-granular `#!?[allow(` (declared limit; F3 deepens); (c) a grandfathered entry whose
  file landed ≤ hard_limit but is still present is exit 2 "remove pending" (§9: "A grandfathered
  entry below 500 → --remove, not --update"), never exit 1 — the four exit-1 causes are the
  closed list; (d) a named path that does not exist or is outside the guard's corpus is usage
  (64): done closes corpus files, the mission names the surviving parts.
- L4 (2026-11-21): wreq_downloader measured at open: 632 lines, grandfathered cap 632 (jq +
  wc -l at 98a26c39) — the S1 quote's "764" was the policy's writing-day figure; the fixture
  asserts the measured count equals the emitted count, never a constant (S9).
