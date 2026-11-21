#!/usr/bin/env bash
# worktree_paths.sh — canonical path identity and live-target registry.
# Sourced by scripts/sweep_orphan_targets.sh; no side effects when sourced.
#
# The live registry is the set of CARGO_TARGET_DIR declarations in the .envrc
# files of live worktrees, NOT the worktree root paths. Compare target paths
# with target paths; comparing them with worktree roots never matches and
# misclassifies active targets as orphans.

# wt_canon <path> — canonicalise with realpath -m. Fail closed: an empty or
# unresolved path is an error, never a string to compare.
wt_canon() {
    local path="${1:-}" out
    if [[ -z "$path" ]]; then
        echo "wt_canon: empty path" >&2
        return 2
    fi
    if ! out="$(realpath -m -- "$path" 2>/dev/null)" || [[ -z "$out" ]]; then
        echo "wt_canon: cannot canonicalise '$path'" >&2
        return 2
    fi
    printf '%s' "$out"
}

# wt_is_within <child> <parent> — canonical equality or descendant.
wt_is_within() {
    local child parent
    child="$(wt_canon "$1")" || return 2
    parent="$(wt_canon "$2")" || return 2
    if [[ "$child" == "$parent" || "$child" == "$parent"/* ]]; then
        return 0
    fi
    return 1
}

# wt_is_reserved <path> <target-cache-root>
# The namespace has FOUR reserved identities: seeds, seeds-webfang,
# quarantine, quarantine-webfang. The suffixed siblings are separate stores
# (campaign artifacts), not descendants of seeds/quarantine, and are excluded
# from collection by documented precedent in odd/tasks/merge-prs-to-main.md.
wt_is_reserved() {
    local candidate="$1" cache_root="$2" root
    for root in seeds seeds-webfang quarantine quarantine-webfang; do
        if wt_is_within "$candidate" "$cache_root/$root"; then
            return 0
        fi
    done
    return 1
}

# wt_envrc_target <file> — value of the LAST CARGO_TARGET_DIR declaration.
# Mirrors parse_envrc_target_decl in check_target_isolation.sh: flexible
# whitespace, one pair of matching quotes stripped, $HOME/${HOME}/~/ expanded.
wt_envrc_target() {
    local file="$1" v
    [[ -f "$file" ]] || return 1
    v="$(sed -n 's/^[[:space:]]*export[[:space:]]\{1,\}CARGO_TARGET_DIR=[[:space:]]*//p' "$file" 2>/dev/null | tail -1)"
    v="${v%"${v##*[![:space:]]}"}"
    if [[ ${#v} -ge 2 ]]; then
        case "$v" in
        \"*\") v="${v:1:${#v}-2}" ;;
        \'*\') v="${v:1:${#v}-2}" ;;
        esac
    fi
    v="${v//\$\{HOME\}/$HOME}"
    v="${v//\$HOME/$HOME}"
    if [[ "$v" == \~/* ]]; then
        v="$HOME/${v:2}"
    elif [[ "$v" == \~ ]]; then
        v="$HOME"
    fi
    if [[ "$v" != /* ]]; then
        v="$(dirname "$file")/$v"
    fi
    wt_canon "$v"
}

# wt_live_targets <repo-root> — print one canonical target path per line for
# every live worktree that declares one. Fail closed on git failure or on a
# declaration that cannot be canonicalised. Detached tool worktrees with no
# .envrc declare nothing and are skipped.
wt_live_targets() {
    local repo="$1" tmp token wt decl
    [[ -d "$repo" ]] || { echo "wt_live_targets: not a directory: $repo" >&2; return 2; }
    tmp="$(mktemp)" || { echo "wt_live_targets: mktemp failed" >&2; return 2; }
    if ! git -C "$repo" worktree list --porcelain -z >"$tmp" 2>/dev/null; then
        echo "wt_live_targets: git worktree enumeration failed" >&2
        rm -f "$tmp"
        return 2
    fi
    local -a tokens=()
    mapfile -d '' -t tokens <"$tmp"
    rm -f "$tmp"
    if [[ ${#tokens[@]} -eq 0 ]]; then
        echo "wt_live_targets: empty worktree registry" >&2
        return 2
    fi
    for token in "${tokens[@]}"; do
        [[ "$token" == worktree\ * ]] || continue
        wt="${token#worktree }"
        decl="$(wt_envrc_target "$wt/.envrc")" || continue
        printf '%s\n' "$decl"
    done
}
