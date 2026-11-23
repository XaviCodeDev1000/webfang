#!/usr/bin/env bash
# identity-attestation.sh — owner-only issuer for the worktree identity exception.
#
# POLICY BARRIER, NOT CRYPTOGRAPHY. The owner token plus an interactive TTY plus a
# typed confirmation keep an agent that has no terminal from issuing its own
# exception. Any process running as the owner can still write the file directly;
# the gate therefore validates the attestation's content, never its provenance.
#
# The issued record carries the intended branch, the current HEAD as the ancestry
# anchor, the external evidence source and a short expiry. The exact staged tree is
# bound by identity-gate.sh at commit time and consumed by post-commit on success.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/../lib/worktree_paths.sh"
# shellcheck disable=SC1091 # resolved relative to SCRIPT_DIR at runtime
source "$SCRIPT_DIR/../lib/identity_attestation.sh"

die() { echo "identity-attestation: $1" >&2; exit 1; }

usage() {
    printf 'usage: identity-attestation.sh --branch <branch> --evidence <source> [--ttl <seconds>]\n'
}

BRANCH=""
EVIDENCE=""
TTL="900"

while (($#)); do
    case "$1" in
    --branch)
        [[ $# -ge 2 ]] || die "--branch needs a value"
        BRANCH="$2"
        shift 2
        ;;
    --evidence)
        [[ $# -ge 2 ]] || die "--evidence needs a value"
        EVIDENCE="$2"
        shift 2
        ;;
    --ttl)
        [[ $# -ge 2 ]] || die "--ttl needs a value"
        TTL="$2"
        shift 2
        ;;
    -h | --help)
        usage
        exit 0
        ;;
    *)
        die "unknown argument: $1"
        ;;
    esac
done

[[ -n "${OWNER_IDENTITY_ATTEST_TOKEN:-}" ]] || die "OWNER_IDENTITY_ATTEST_TOKEN must be set by the owner"
[[ -t 0 ]] || die "interactive TTY required"
[[ -n "$BRANCH" ]] || { usage >&2; die "--branch is required"; }
[[ -n "$EVIDENCE" ]] || { usage >&2; die "--evidence is required"; }
[[ "$TTL" =~ ^[0-9]+$ ]] || die "--ttl must be a positive integer"
(( TTL > 0 )) || die "--ttl must be a positive integer"

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || die "not inside a git worktree"
ROOT_CANON="$(wt_canon "$ROOT")" || die "cannot canonicalise worktree root"
HEAD_BRANCH="$(git symbolic-ref --quiet --short HEAD 2>/dev/null || true)"
[[ -n "$HEAD_BRANCH" ]] || die "detached HEAD cannot be attested"
[[ "$HEAD_BRANCH" == "$BRANCH" ]] || die "intended branch '$BRANCH' != HEAD branch '$HEAD_BRANCH'"
[[ "$(basename "$ROOT_CANON")" != "${BRANCH//\//-}" ]] || die "directory matches branch; no exception is needed"

PATH_FILE="$(ia_path)" || die "cannot resolve attestation path"
[[ ! -e "$PATH_FILE" ]] || die "an attestation already exists at $PATH_FILE (it is consumed after a successful commit)"

CONFIRM="AUTHORIZE WORKTREE IDENTITY EXCEPTION FOR $BRANCH"
printf 'Type exactly: %s\n' "$CONFIRM"
read -r ANSWER || die "confirmation was not entered"
[[ "$ANSWER" == "$CONFIRM" ]] || die "confirmation mismatch"

ANCHORED="$(git rev-parse HEAD 2>/dev/null)" || die "cannot resolve HEAD"
EXPIRES=$(( $(date +%s) + TTL ))
NONCE="$(od -An -tx1 -N8 /dev/urandom | tr -d ' \n')"
[[ -n "$NONCE" ]] || die "cannot mint nonce"

TMP="$(mktemp "${PATH_FILE}.XXXXXX")" || die "cannot create temporary attestation"
umask 077
{
    printf 'version=1\n'
    printf 'branch=%s\n' "$BRANCH"
    printf 'anchored_head=%s\n' "$ANCHORED"
    printf 'evidence=%s\n' "$EVIDENCE"
    printf 'expires_at=%s\n' "$EXPIRES"
    printf 'nonce=%s\n' "$NONCE"
} >"$TMP" || { rm -f "$TMP"; die "cannot write temporary attestation"; }
mv -f "$TMP" "$PATH_FILE" || { rm -f "$TMP"; die "cannot install attestation"; }

ia_validate "$PATH_FILE" "$BRANCH" "$ROOT_CANON" || die "issued attestation failed self-validation"

printf 'identity-attestation: issued for %s (expires in %ss) at %s\n' "$BRANCH" "$TTL" "$PATH_FILE"
