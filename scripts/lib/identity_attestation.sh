#!/usr/bin/env bash
# identity_attestation.sh — shared storage and validation for the identity exception.
# The attestation lives in the per-worktree git dir: it is never versioned and never
# placed in the shared git common dir, so one worktree cannot spend another's exception.
# This is a policy barrier backed by an owner-only interactive issuer, not cryptography:
# any process with the owner's shell can write the file. The gate validates its CONTENT
# (branch, ancestry, expiry, exact staged tree) regardless of who wrote it.

ia_path() {
    local git_dir
    git_dir="$(git rev-parse --absolute-git-dir 2>/dev/null)" || return 1
    [[ -n "$git_dir" ]] || return 1
    printf '%s/webfang-identity-attestation' "$git_dir"
}

ia_field() {
    local file="$1" key="$2"
    [[ -f "$file" ]] || return 1
    awk -v k="$key" 'index($0, k "=") == 1 { sub(k "=", ""); print; exit }' "$file"
}

ia_set_field() {
    local file="$1" key="$2" value="$3" tmp
    [[ -f "$file" ]] || return 1
    tmp="$(mktemp "${file}.XXXXXX")" || return 1
    if ! awk -v k="$key" -v v="$value" '
        index($0, k "=") == 1 { print k "=" v; seen=1; next }
        { print }
        END { if (!seen) print k "=" v }
    ' "$file" >"$tmp"; then
        rm -f "$tmp"
        return 1
    fi
    if ! mv -f "$tmp" "$file"; then
        rm -f "$tmp"
        return 1
    fi
}

ia_validate() {
    local file="$1" branch="$2" worktree_canon="$3"
    local version recorded anchored expires evidence now sha

    [[ -f "$file" ]] || { echo "identity-attestation: attestation file is missing" >&2; return 1; }

    version="$(ia_field "$file" version)" || return 1
    [[ "$version" == "1" ]] || { echo "identity-attestation: unsupported version '$version'" >&2; return 1; }

    recorded="$(ia_field "$file" branch)" || return 1
    [[ "$recorded" == "$branch" ]] || {
        echo "identity-attestation: recorded branch '$recorded' != HEAD branch '$branch'" >&2
        return 1
    }

    expires="$(ia_field "$file" expires_at)" || return 1
    [[ "$expires" =~ ^[0-9]+$ ]] || { echo "identity-attestation: invalid expiry '$expires'" >&2; return 1; }
    now="$(date +%s)" || return 1
    (( expires > now )) || { echo "identity-attestation: expired at $expires (now $now)" >&2; return 1; }

    evidence="$(ia_field "$file" evidence)" || return 1
    [[ -n "$evidence" ]] || { echo "identity-attestation: empty evidence" >&2; return 1; }

    anchored="$(ia_field "$file" anchored_head)" || return 1
    [[ "$anchored" =~ ^[0-9a-f]{40,64}$ ]] || {
        echo "identity-attestation: invalid anchored_head '$anchored'" >&2
        return 1
    }
    sha="$(git rev-parse --verify --quiet "${anchored}^{commit}" 2>/dev/null)" || {
        echo "identity-attestation: anchored_head is not a commit in this repository" >&2
        return 1
    }
    if ! git merge-base --is-ancestor "$sha" HEAD 2>/dev/null; then
        echo "identity-attestation: anchored_head is not an ancestor of HEAD" >&2
        return 1
    fi

    # Condition 2: path -> branch binding, from git's own worktree registry.
    if ! git worktree list --porcelain | awk -v p="$worktree_canon" -v b="refs/heads/$branch" '
        $0 == "worktree " p { here=1; next }
        here && $0 == "branch " b { ok=1; exit }
        here && /^worktree / { exit }
        END { exit(ok ? 0 : 1) }
    '; then
        echo "identity-attestation: git worktree list does not bind this path to '$branch'" >&2
        return 1
    fi

    return 0
}
