# Sources — webfang-file-splits

This skill is the actionable layer. The full OLAS program is the source of truth in the repo; the
expert/official comparison and the alignment decision live in Engram. Do not copy either here.

## Engram (comparison / decision / history)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| Official Rust guidance vs OLAS (no file limit; function-level lints; Google/Microsoft guidance) | `Official Rust guidance file size OLAS plan 500 lines` | #3000 | project |
| OLAS aligned with official guidance (co-equal closure, crate-boundary question, churn ordering) | `OLAS plan aligned with official Rust guidance` | #3001 | project |
| waf_engine split by real seams, out of the baseline | `waf_engine split seams file size reduction baseline` | #849 | project |

Resolve with `engram search "<query>" --project webfang` or `mem_search`.

## Repo sources of truth (normative)

- `../../../odd/tasks/file-size-reduction.md` — OLAS program: principles, waves, per-file method,
  two-hat review protocol, governance. Read the checked-out file; it is the source of truth.
- `../../../scripts/quality-baselines.json` — `file_size_rust.grandfathered`: the mutable list and
  each file's `notes`; never copy it into prose.
- `../../../scripts/check_file_size.sh` — the ratchet guard (`--update` / `--remove`).
- `../../../scripts/check_duplication.sh` — pinned jscpd 5.4.0 and `cx` measurement.
- `../../../AGENTS.md` — § Architecture (no new file over ~500 lines) and § Commits, PRs and CI.
- `../../../scripts/check-topology.sh` — sequential delivery (no stacked PRs).
