#!/usr/bin/env bash
# check_workflow_concurrency_hygiene.sh
#
# CI gate for one workflow-concurrency class that NO compiler or clippy lint
# catches and that only fails at runtime, by destroying another run.
# `scripts/check_concurrency_lints.sh` lints Rust semaphores (CC-L1/CC-L2); this
# one lints the Actions `concurrency:` keyword, which is a one-line edit.
#
# THE TRAP, on evidence. On 2026-11-12 a `workflow_dispatch` of ci.yml at
# 17:46:08 was killed at 128 of its 240-minute budget by a second dispatch at
# 17:55:51. The `Miri (core)` job was still running: the run did not fail and
# did not time out, so its result is permanently UNKNOWN — worse than a red
# check, because 4 hours of runner time produced no information.
#
# The mechanism, precisely:
#
#   * `github.head_ref` is defined ONLY on `pull_request`. On `workflow_dispatch`
#     it is an empty string, so a group of the shape
#         ${{ github.workflow }}-${{ github.event_name }}-${{ github.head_ref || github.ref }}
#     falls back to `github.ref` and every manual run shares ONE group.
#   * A shared group only KILLS a run when `cancel-in-progress` is true for that
#     event. With `cancel-in-progress: false` (the default) the newer run is
#     QUEUED until the older finishes — it waits, it does not destroy.
#
# WHAT THIS GATE ENFORCES (W1)
#
#   A workflow that can be dispatched manually, whose concurrency group has no
#   per-run discriminator, may not set `cancel-in-progress` to a value that is
#   true for `workflow_dispatch`. Both halves must hold, because either alone
#   is harmless: a constant group with cancellation OFF only serializes, and a
#   per-run discriminator with cancellation ON is exactly the fix.
#
# WHAT THIS GATE DOES NOT ENFORCE, AND WHY — each is a decision, not an oversight
#
#   * It does NOT require uniqueness where `cancel-in-progress` is false. The
#     repository sets that deliberately, with reasons written at the call site
#     (`benches.yml`: "never kill a nightly mid-measurement"; `ci-health.yml`:
#     "never drop a tracking update mid-write"). Demanding uniqueness there
#     would override an informed decision to buy a queueing delay instead of a
#     cancellation. Such workflows are REPORTED, not failed.
#
#   * It does NOT require a workflow to declare `concurrency:` at all. A
#     workflow with no block has no cancellation and no queueing; its runs
#     simply proceed. Whether two of its runs may safely overlap is a property
#     of what they write, which this gate cannot see.
#
#   * It reads the group and the `cancel-in-progress` expression as TEXT. It does
#     not evaluate expressions: a group that names `workflow_dispatch` and
#     `github.run_id` in unrelated branches passes here, and a
#     `cancel-in-progress` expression that is true for `workflow_dispatch` by
#     some route other than naming it is read as safe. `actionlint` covers
#     whether the expression is well formed; this covers the shape.
#
# Scope: `.github/workflows/*.yml` of the repository. Override the root with
# WORKFLOW_CONCURRENCY_ROOT.

set -euo pipefail

WORKFLOW_CONCURRENCY_ROOT="${WORKFLOW_CONCURRENCY_ROOT:-$(git rev-parse --show-toplevel)}"
export WORKFLOW_CONCURRENCY_ROOT

python3 - <<'PY'
import os
import sys

import yaml

root = os.environ["WORKFLOW_CONCURRENCY_ROOT"]
wf_dir = os.path.join(root, ".github", "workflows")

if not os.path.isdir(wf_dir):
    print(f"FAIL: workflow directory not found: {wf_dir}", file=sys.stderr)
    sys.exit(1)

DISCRIMINATORS = ("github.run_id", "github.run_attempt")


def triggers_of(doc):
    # YAML 1.1 parses the bare key `on:` as boolean True, so the triggers live
    # under either `on` or True. Reading both is what keeps this from reporting
    # "no triggers" on every file.
    node = doc.get("on", doc.get(True))
    if node is None:
        return []
    if isinstance(node, str):
        return [node]
    if isinstance(node, list):
        return [t for t in node if isinstance(t, str)]
    if isinstance(node, dict):
        return [k for k in node if isinstance(k, str)]
    return []


def cancels_on_dispatch(conc):
    """True when `cancel-in-progress` is true for the workflow_dispatch event.

    Read as text, which is the limit this script states above:
      * a literal true cancels on every event, dispatch included;
      * an expression that names workflow_dispatch is assumed CAN be true there
        — that is the conservative reading for a gate that exists to prevent a
        cancellation;
      * anything else (an expression naming only pull_request, or an absent
        key, whose default is false) does not cancel a manual run.
    """
    if not isinstance(conc, dict) or "cancel-in-progress" not in conc:
        return False
    value = conc["cancel-in-progress"]
    if value is True:
        return True
    if isinstance(value, str):
        return "workflow_dispatch" in value
    return False


failures = []
queueing = []
checked = 0

for name in sorted(os.listdir(wf_dir)):
    if not name.endswith((".yml", ".yaml")):
        continue
    path = os.path.join(wf_dir, name)
    checked += 1
    with open(path, encoding="utf-8") as fh:
        doc = yaml.safe_load(fh)
    if not isinstance(doc, dict):
        failures.append(f"{name}: not a YAML mapping")
        continue

    if "workflow_dispatch" not in triggers_of(doc):
        continue
    conc = doc.get("concurrency")
    if not isinstance(conc, dict):
        continue  # out of scope by decision: no block means no cancellation
    group = conc.get("group")
    if not isinstance(group, str):
        group = ""

    has_discriminator = any(d in group for d in DISCRIMINATORS)
    if has_discriminator:
        continue

    if cancels_on_dispatch(conc):
        failures.append(
            f"{name}: can be dispatched manually and its concurrency group "
            f"({group!r}) has no per-run discriminator, while "
            "cancel-in-progress is true for workflow_dispatch. github.head_ref "
            "is EMPTY on workflow_dispatch (it is only defined on pull_request), "
            "so the group collapses to github.ref and every manual run shares "
            "ONE group: the newest run CANCELS the older one, silently, and its "
            "result is lost rather than failed. Scope github.run_id to that "
            "event, as .github/workflows/ci.yml does."
        )
    else:
        queueing.append(name)

if checked == 0:
    print(f"FAIL: no workflows found under {wf_dir}", file=sys.stderr)
    sys.exit(1)

for name in queueing:
    print(
        f"NOTE: {name}: dispatchable with a constant concurrency group, but "
        "cancel-in-progress is false for workflow_dispatch — manual runs "
        "QUEUE behind each other instead of destroying each other. Left as the "
        "repository's deliberate choice; this gate does not demand uniqueness "
        "where cancellation is off."
    )

for line in failures:
    print(f"FAIL: {line}", file=sys.stderr)

if failures:
    print(
        f"\nFAIL: workflow concurrency hygiene: {len(failures)} workflow(s) "
        "break W1",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"OK: workflow concurrency hygiene holds over {checked} workflow(s)")
PY
