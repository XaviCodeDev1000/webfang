#!/usr/bin/env bash
set -euo pipefail
#
# fake_gh for test_ci_queue_metrics.sh — serves fixture payloads instead of
# the network. Dispatches on the gh subcommand:
#   repo view                       -> the fixture repo identity
#   run list (no --event)           -> fixture runs.json (the --branch lane)
#   run list --event merge_group    -> fixture merge_runs.json, or [] when the
#                                      fixture predates the merge-queue lane
#   api repos/<repo>/actions/runs/N/jobs -> fixture jobs-N.json (per run id)
#   api repos/<repo>/issues/N/timeline   -> fixture timeline-N.json (per PR)
set -euo pipefail

FAKE_GH_DIR="${FAKE_GH_DIR:?FAKE_GH_DIR must point at the fixture directory}"

case "${1:-}" in
  repo)
    # `gh repo view --json nameWithOwner`
    cat "$FAKE_GH_DIR/repo.json"
    exit 0
    ;;
  run)
    # `gh run list --workflow ... [--branch ... | --event merge_group] ...`
    for arg in "$@"; do
      if [ "$arg" = "merge_group" ]; then
        if [ -f "$FAKE_GH_DIR/merge_runs.json" ]; then
          cat "$FAKE_GH_DIR/merge_runs.json"
        else
          echo '[]'
        fi
        exit 0
      fi
    done
    cat "$FAKE_GH_DIR/runs.json"
    exit 0
    ;;
  api)
    # `gh api repos/<repo>/actions/runs/<id>/jobs?per_page=100`
    # or `gh api repos/<repo>/issues/<pr>/timeline?per_page=100`.
    # $2 is the path with its query; strip the query first.
    path="${2%%\?*}"
    case "$path" in
      */actions/runs/*/jobs)
        run_id="${path%/jobs}"
        run_id="${run_id##*/}"
        fixture="$FAKE_GH_DIR/jobs-$run_id.json"
        ;;
      */issues/*/timeline)
        pr="${path%/timeline}"
        pr="${pr##*/}"
        fixture="$FAKE_GH_DIR/timeline-$pr.json"
        ;;
      *)
        echo "error: fake gh: unsupported api path '$path'" >&2
        exit 2
        ;;
    esac
    if [ -f "$fixture" ]; then
      cat "$fixture"
    else
      echo "error: fake gh has no fixture for $path" >&2
      exit 1
    fi
    exit 0
    ;;
  *)
    echo "error: fake gh: unsupported subcommand '${1:-}'" >&2
    exit 2
    ;;
esac
