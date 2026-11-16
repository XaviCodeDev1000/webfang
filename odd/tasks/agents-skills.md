# Mission: project skills that complement Engram

Branch: `docs/agents-skills` · type: `docs` · worktree: `webfang-worktrees/docs-agents-skills`

## Why

`AGENTS.md` regrew to 862 lines because agents append operational runbook prose on every commit.
The owner wants the important operational knowledge to also live as trigger-loaded project skills
in `.agents/skills/`, so agents are guided in the repo's way of working without bloating the
always-loaded router. Owner decision: create only skills that help this project and **complement
Engram** (not replace it).

## Complement model (no duplication)

Each skill is a thin runtime contract (activation trigger, hard rules the compiler cannot catch,
decision gates, execution steps, output contract). It points to Engram for the detailed runbook /
history / diagnosis and to local repo sources of truth. It does NOT copy the Engram runbook
verbatim. Honors obs #370 (knowledge lives in Engram; repo holds rules + sources of truth) and the
routerization spec S2/S5. Format follows the bundled `skill-creator` guide (no repo
`docs/skill-style-guide.md`): fixed section order, body under the 1000-token hard max, detail in
local `references/`.

## Scope (owner-selected)

Four skills, each `SKILL.md` + `references/sources.md`:

- `webfang-worktree-ops` — worktree lifecycle; complements Engram #2307.
- `webfang-ci-gate` — local gate + coverage report; complements Engram #2980/#2309.
- `webfang-testing` — harness + `webfang_path()` + insta + six-node; repo docs are the source.
- `webfang-fetch-path` — mandatory fetch guard order; AGENTS.md table stays normative.

`webfang-pr-merge` deliberately out (overlaps generic `branch-pr` / `work-unit-commits`).

## Changes

- `.agents/skills/<four>/SKILL.md` + `references/sources.md` — new.
- `.gitignore` — four `!` adoption lines (convention #1869: one per adopted skill).
- `AGENTS.md` § Skills and CLIs — append the four names to the repo-scope list (only AGENTS.md edit).

## Verification

- Frontmatter complete, single-line quoted trigger-first description ≤250 chars; sections in order;
  bodies ~2.8–3.1 KiB (under hard max, leaner than the existing `webfang-architecture`).
- Cited symbols re-resolved via codedb against the current tree (v8.0.0): `fetch_inner` now also in
  `application/http_client/client.rs`; `build_scrape_rate_limiter` moved to
  `application/batch/processor.rs` — references reflect current paths.
- Engram pointer queries resolve (#2307, #2980 verified via `engram search`).
- `gentle-ai skill-registry refresh` lists the four skills (project scope, exact paths).
- `bash scripts/ci_fast_gate.sh` GREEN (docs+ci lanes, PASS=11 FAIL=0); coverage report read.

## Related paused track

The AGENTS.md prune to ≤650 lines / ≤40 KiB + the size/structure guard (tasks #3–#6) is a separate
sequential slice; this skills PR is disjoint from its runbook edits (only the skills-list line
overlaps), so sequential merge is clean.