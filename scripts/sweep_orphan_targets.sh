#!/usr/bin/env bash
# sweep_orphan_targets.sh — classify CARGO_TARGET_DIR leftovers.
#
# Dry-run by default: it never deletes unless --execute AND the owner-provided
# SWEEP_CONFIRM_TOKEN are present. The live set comes from the CARGO_TARGET_DIR
# declarations of live worktrees, not from worktree root paths; reserved
# identities are matched by canonical path, never by string prefix.
#
# Usage: scripts/sweep_orphan_targets.sh [--idle 72h] [--execute]
#        [--target-cache <dir>] [--repo <dir>] [--report-file <file>]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/lib/worktree_paths.sh"

IDLE="72h"
MODE="dry-run"
TARGET_CACHE_ARG=""
REPO_ARG=""
REPORT_FILE=""

usage() {
    cat <<'EOF'
sweep_orphan_targets.sh — classify orphan cargo target dirs (dry-run default)

Options:
  --idle DUR           minimum age of the newest file in a target (e.g. 48h, 7d)
  --execute            actually delete candidates (requires SWEEP_CONFIRM_TOKEN)
  --target-cache DIR   cache root to scan (default: $HOME/.cache/cargo-target)
  --repo DIR           git worktree root used for the live registry (default: cwd toplevel)
  --report-file FILE   also write the JSON report to FILE
  -h, --help           this text
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
    --idle)
        [[ $# -ge 2 ]] || { echo "error: --idle requires a value" >&2; exit 2; }
        IDLE="$2"; shift 2 ;;
    --execute) MODE="execute"; shift ;;
    --target-cache)
        [[ $# -ge 2 ]] || { echo "error: --target-cache requires a value" >&2; exit 2; }
        TARGET_CACHE_ARG="$2"; shift 2 ;;
    --repo)
        [[ $# -ge 2 ]] || { echo "error: --repo requires a value" >&2; exit 2; }
        REPO_ARG="$2"; shift 2 ;;
    --report-file)
        [[ $# -ge 2 ]] || { echo "error: --report-file requires a value" >&2; exit 2; }
        REPORT_FILE="$2"; shift 2 ;;
    -h | --help) usage; exit 0 ;;
    *) echo "error: unknown argument '$1'" >&2; exit 2 ;;
    esac
done

if [[ -z "$TARGET_CACHE_ARG" ]]; then
    TARGET_CACHE_ARG="${WEBFANG_TARGET_CACHE:-$HOME/.cache/cargo-target}"
fi
if [[ -z "$REPO_ARG" ]]; then
    REPO_ARG="$(git rev-parse --show-toplevel 2>/dev/null)" || {
        echo "error: not inside a git worktree; pass --repo" >&2
        exit 2
    }
fi

TARGET_CACHE="$(wt_canon "$TARGET_CACHE_ARG")" || exit 2
REPO="$(wt_canon "$REPO_ARG")" || exit 2
[[ -d "$TARGET_CACHE" ]] || { echo "error: target cache not found: $TARGET_CACHE" >&2; exit 2; }
[[ -d "$REPO/.git" || -f "$REPO/.git" ]] || { echo "error: not a git root: $REPO" >&2; exit 2; }

case "$IDLE" in
*h) IDLE_SECONDS="$((${IDLE%h} * 3600))" ;;
*d) IDLE_SECONDS="$((${IDLE%d} * 86400))" ;;
*) echo "error: --idle must end in h or d" >&2; exit 2 ;;
esac
[[ "$IDLE_SECONDS" -gt 0 ]] || { echo "error: --idle must be positive" >&2; exit 2; }

if [[ "$MODE" == "execute" && -z "${SWEEP_CONFIRM_TOKEN:-}" ]]; then
    echo "error: --execute requires SWEEP_CONFIRM_TOKEN set by the owner" >&2
    exit 2
fi

if ! command -v lsof >/dev/null 2>&1; then
    echo "error: lsof is required; refusing to classify targets without an in-use check" >&2
    exit 2
fi

# Live registry: canonical TARGET paths declared by live worktrees.
# A temp file is used instead of process substitution so a git failure cannot
# be swallowed as an empty registry (fail closed).
LIVE_TMP="$(mktemp)" || exit 2
if ! wt_live_targets "$REPO" >"$LIVE_TMP"; then
    rm -f "$LIVE_TMP"
    exit 2
fi
declare -A LIVE_TARGETS=()
while IFS= read -r live; do
    [[ -n "$live" ]] && LIVE_TARGETS["$live"]=1
done <"$LIVE_TMP"
rm -f "$LIVE_TMP"

newest_file_mtime() {
    local dir="$1" stamp
    stamp="$(find "$dir" -type f -printf '%T@\n' 2>/dev/null | sort -rn | head -1)"
    [[ -n "$stamp" ]] || return 1
    printf '%s' "${stamp%%.*}"
}

now="$(date +%s)"
declare -a CANDIDATES=()
scanned=0 reserved_count=0 live_count=0 protected_count=0
total_bytes=0

for dir in "$TARGET_CACHE"/*/; do
    [[ -d "$dir" ]] || continue
    scanned=$((scanned + 1))
    name="$(basename "$dir")"
    dir_canon="$(wt_canon "$dir")" || exit 2

    if wt_is_reserved "$dir_canon" "$TARGET_CACHE"; then
        reserved_count=$((reserved_count + 1))
        continue
    fi
    if [[ -n "${LIVE_TARGETS[$dir_canon]:-}" ]]; then
        live_count=$((live_count + 1))
        continue
    fi
    # Only directories carrying Cargo's own markers are treated as targets.
    if [[ ! -f "$dir/CACHEDIR.TAG" && ! -f "$dir/.rustc_info.json" ]]; then
        protected_count=$((protected_count + 1))
        continue
    fi
    if lsof +D "$dir" >/dev/null 2>&1; then
        protected_count=$((protected_count + 1))
        continue
    fi
    newest="$(newest_file_mtime "$dir")" || { protected_count=$((protected_count + 1)); continue; }
    age=$((now - newest))
    if (( age < IDLE_SECONDS )); then
        protected_count=$((protected_count + 1))
        continue
    fi
    size="$(du -sb "$dir" 2>/dev/null | cut -f1)"
    [[ -n "$size" ]] || { protected_count=$((protected_count + 1)); continue; }
    CANDIDATES+=("$name")
    total_bytes=$((total_bytes + size))
done

json() {
    printf '{\n'
    printf '  "mode": "%s",\n' "$MODE"
    printf '  "target_cache": "%s",\n' "$TARGET_CACHE"
    printf '  "idle": "%s",\n' "$IDLE"
    printf '  "scanned": %d,\n' "$scanned"
    printf '  "reserved": %d,\n' "$reserved_count"
    printf '  "live": %d,\n' "$live_count"
    printf '  "protected": %d,\n' "$protected_count"
    printf '  "orphaned": %d,\n' "${#CANDIDATES[@]}"
    printf '  "reclaimable_bytes": %d,\n' "$total_bytes"
    printf '  "candidates": ['
    local first=1 candidate
    for candidate in "${CANDIDATES[@]}"; do
        if [[ $first -eq 0 ]]; then printf ','; fi
        first=0
        printf '\n    "%s"' "${candidate//\"/\\\"}"
    done
    if [[ $first -eq 0 ]]; then printf '\n  '; fi
    printf ']\n}\n'
}

if [[ -n "$REPORT_FILE" ]]; then
    json | tee "$REPORT_FILE"
else
    json
fi

if [[ "$MODE" == "dry-run" ]]; then
    exit 0
fi

# Re-read the live registry at delete time: a worktree may have appeared
# between the scan and the token-gated execution. Fail closed if git fails.
FRESH_TMP="$(mktemp)" || exit 2
if ! wt_live_targets "$REPO" >"$FRESH_TMP"; then
    rm -f "$FRESH_TMP"
    echo "error: live registry unreadable at delete time; refusing" >&2
    exit 2
fi
declare -A FRESH_LIVE=()
while IFS= read -r live; do
    [[ -n "$live" ]] && FRESH_LIVE["$live"]=1
done <"$FRESH_TMP"
rm -f "$FRESH_TMP"
del_now="$(date +%s)"

failures=0
deleted=0
for name in "${CANDIDATES[@]}"; do
    dir="$TARGET_CACHE/$name"
    [[ -d "$dir" ]] || continue
    dir_canon="$(wt_canon "$dir")" || exit 2
    # Re-check every guard at delete time, not just the scan-time snapshot.
    if wt_is_reserved "$dir_canon" "$TARGET_CACHE" || [[ -n "${FRESH_LIVE[$dir_canon]:-}" ]]; then
        echo "skip (state changed): $dir" >&2
        continue
    fi
    if lsof +D "$dir" >/dev/null 2>&1; then
        echo "skip (in use at delete time): $dir" >&2
        continue
    fi
    newest="$(newest_file_mtime "$dir")" || {
        echo "skip (no files at delete time): $dir" >&2
        continue
    }
    if (( del_now - newest < IDLE_SECONDS )); then
        echo "skip (recent at delete time): $dir" >&2
        continue
    fi
    if rm -rf -- "$dir" 2>/dev/null; then
        deleted=$((deleted + 1))
    else
        echo "delete failed: $dir" >&2
        failures=$((failures + 1))
    fi
done
printf 'deleted=%d failures=%d\n' "$deleted" "$failures"
if (( failures > 0 )); then exit 2; fi
exit 0
