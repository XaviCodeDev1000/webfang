#!/usr/bin/env bash
#
# Ensure a GitHub Release exists and is COMPLETE for a release tag.
#
# Idempotent by construction: a Release that already carries every expected asset
# is a no-op, so this is safe to call both from the push-triggered path and from
# the reconciliation sweep. A Release left incomplete by a failed upload is
# re-dispatched rather than skipped.
#
# Why the retry (webfang#1484): the failure class here is transient — a GitHub
# API hiccup, or `actions: write` regressing mid-flight. With a single
# `gh workflow run`, a failure left the tag with no binaries and nothing to
# observe, because a dispatch that never happened produces no workflow run.
# A terminal failure is now loud and names the tag.
#
# The tag travels as an INPUT, and the dispatch deliberately does NOT pin `--ref`.
# What pins the artifact is the input: release.yml points BOTH of its checkouts at
# `inputs.tag || github.ref_name`. Passing `--ref <tag>` would instead make the run
# execute THAT TAG'S HISTORICAL release.yml — and this helper is shared with the
# reconciliation sweep, whose entire subject is old tags. A sweep for v2.0.0 would
# then run v2.0.0's release.yml, turning the pipeline into a function of when the
# tag was cut (and reintroducing pre-hardening definitions). Dispatching from the
# default branch always runs the current definition, hardened, with the input
# pinning the build. This is the same choice the v2.1.1 backfill made by hand.
#
# L1 Provenance: the dispatch now also passes `expected_sha` (the commit the tag
# resolves to) so release.yml's preflight can verify L1.1 Identity (Shape 2:
# default-branch dispatch requires PROV_EXPECTED_SHA).
#
# WHY THE COMPLETENESS CHECK READS ASSETS BY RELEASE ID (webfang#1535):
# it used to read them from the by-tag/by-name view (`gh release view --json
# assets`, the same data as GET .../releases/tags/<tag>'s embedded .assets).
# For release id 394502300 / tag v2.3.0 that view returned an EMPTY .assets
# array while the release actually had all 5 assets: GET .../releases,
# GET .../releases/{id} and GET .../releases/{id}/assets each returned 5
# (control v2.2.0: 5 everywhere). A COMPLETE release was therefore reported
# as incomplete and dispatched spuriously (run 35844323775), which then failed
# closed at L1 preflight for a missing expected_sha — that fail-closed was
# correct, but the dispatch should never have happened.
# So: by-tag is used ONLY to resolve the release id (it returned a valid id
# even when its embedded assets were empty), and assets are read EXCLUSIVELY
# from GET /repos/$GITHUB_REPOSITORY/releases/<id>/assets — the one endpoint
# measured to return the full list. Three states stay distinct: no release ->
# "does not exist yet"; resolvable id with missing assets -> "incomplete";
# resolvable id with every expected asset -> "already complete", exit 0 with
# NO dispatch. A present-but-empty assets array on a resolvable id is
# "incomplete", never "release missing".
#
# WHY COMPLETENESS IS JUDGED PER ERA (webfang#2493):
# the expected inventory grew over time — keyless signing added a
# <asset>.sigstore.json bundle for every asset, and the MCP archives were added
# later still. Judging an old release against today's inventory calls a release
# that was complete on the day it shipped "incomplete" forever, and the sweep
# re-dispatched it daily: 33 doomed runs a day, 38 of the last 40 release.yml
# runs red. A release is therefore judged against the inventory of ITS era; the
# cut-overs are recorded below and measured against the published releases.
#
# A tag from before REBUILDABLE_FROM_TAG is not dispatched at all: its tree pins
# its own toolchain, the current pipeline cannot build it, so an incomplete or
# missing Release there is a PERMANENT gap. It is named, counted and left alone
# — never silently called complete.
#
# Usage: ensure-release.sh <tag> [expected_sha]
set -euo pipefail

TAG="${1:-}"
EXPECTED_SHA="${2:-}"
if [[ -z "$TAG" ]]; then
  echo "usage: $(basename "$0") <tag> [expected_sha]" >&2
  exit 2
fi

# The assets release.yml must publish for a release to count as complete: the
# build matrix, the checksums manifest, the SBOM and the third-party notices
# bundle (RELEASE_EXPECTED_ASSETS in .github/workflows/release.yml is the same
# inventory, transcribed once there for the sign/verify/publish checklists).
# This is the CURRENT era's inventory: the push path and the sweep cannot
# disagree about it — and its wiring is enforced by the harness in
# scripts/test_release_reconcile.sh, which diffs both lists.
EXPECTED_ASSETS=(
  webfang-x86_64-unknown-linux-gnu.tar.gz
  webfang-aarch64-unknown-linux-gnu.tar.gz
  webfang-aarch64-apple-darwin.tar.gz
  webfang-x86_64-pc-windows-msvc.zip
  webfang-mcp-x86_64-unknown-linux-gnu.tar.gz
  webfang-mcp-aarch64-unknown-linux-gnu.tar.gz
  webfang-mcp-aarch64-apple-darwin.tar.gz
  webfang-mcp-x86_64-pc-windows-msvc.zip
  webfang-mcp-stdio-x86_64-unknown-linux-gnu.tar.gz
  webfang-mcp-stdio-aarch64-unknown-linux-gnu.tar.gz
  webfang-mcp-stdio-aarch64-apple-darwin.tar.gz
  webfang-mcp-stdio-x86_64-pc-windows-msvc.zip
  SHA256SUMS.txt
  webfang.spdx.json
  THIRD_PARTY_NOTICES.txt
)
# ─── Inventory eras: judge a release by ITS era, not by today's ──────────────
# EXPECTED_ASSETS above is what the CURRENT pipeline publishes. Applying it to
# history demands assets that did not exist when the release was cut, so a
# release that was complete on the day it shipped reads as "incomplete"
# forever (webfang#2493).
#
# The cut-overs are RECORDED and measured against the published releases, never
# inferred from the release being judged: inferring "what it shipped" from
# "what it has" makes the predicate vacuous.
#
#   v2.7.1  first release publishing a <asset>.sigstore.json bundle for every
#           asset; v2.1.0-v2.4.1 ship 5 assets with no bundles at all.
#   v7.0.0  first release publishing the webfang-mcp and webfang-mcp-stdio
#           archives; v2.7.1-v6.0.0 ship 7 assets + 7 bundles (14).
#
# A tag older than every recorded era is judged against the OLDEST one. That is
# the conservative direction: it can over-report a missing asset, never hide
# one behind a shorter list.
SIGNED_BUNDLES_FROM_TAG="v2.7.1"
MCP_ASSETS_FROM_TAG="v7.0.0"

# The oldest tag whose tree the CURRENT pipeline can rebuild — a property of the
# workflow, recorded once. Every tag up to v6.0.0 pins channel = "1.88.0" in its
# own rust-toolchain.toml, and rustup's toolchain-file override outranks the
# toolchain the workflow installs, so the cross target lands where it never was
# (E0463). v7.0.0 is the first tag pinning 1.91.1, the line release.yml installs.
#
# Below this floor an incomplete or missing Release is a PERMANENT gap: no
# dispatch can fill it. The sweep reports it and leaves it alone; it is never
# counted as complete.
REBUILDABLE_FROM_TAG="v7.0.0"

# Pre-signing era: the CLI archives and the checksums manifest. Written out
# instead of "EXPECTED_ASSETS minus the newer names" so an asset added to the
# current era can never leak backwards into the historical predicate.
PRE_SIGNING_ERA_ASSETS=(
  webfang-x86_64-unknown-linux-gnu.tar.gz
  webfang-aarch64-unknown-linux-gnu.tar.gz
  webfang-aarch64-apple-darwin.tar.gz
  webfang-x86_64-pc-windows-msvc.zip
  SHA256SUMS.txt
)
# Signed era: the same archives plus the SBOM and the notices bundle.
SIGNED_ERA_ASSETS=(
  "${PRE_SIGNING_ERA_ASSETS[@]}"
  webfang.spdx.json
  THIRD_PARTY_NOTICES.txt
)

# tag_at_or_after <tag> <floor> — semver ordering without git, so the predicate
# works on any checkout and on a tag that is not an ancestor of HEAD.
tag_at_or_after() {
  [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" == "$2" ]]
}

# era_assets <tag> — the canonical assets of the tag's era, one per line.
era_assets() {
  local tag="$1"
  if tag_at_or_after "$tag" "$MCP_ASSETS_FROM_TAG"; then
    printf '%s\n' "${EXPECTED_ASSETS[@]}"
  elif tag_at_or_after "$tag" "$SIGNED_BUNDLES_FROM_TAG"; then
    printf '%s\n' "${SIGNED_ERA_ASSETS[@]}"
  else
    printf '%s\n' "${PRE_SIGNING_ERA_ASSETS[@]}"
  fi
}

# required_asset_names <tag> — the era's assets, each with its cosign bundle
# when the tag's era signed at all (release.yml uploads <asset>.sigstore.json
# for every asset — audit B8: assets without bundles are not complete either).
required_asset_names() {
  local tag="$1" a
  while IFS= read -r a; do
    if tag_at_or_after "$tag" "$SIGNED_BUNDLES_FROM_TAG"; then
      printf '%s\n%s\n' "$a" "$a.sigstore.json"
    else
      printf '%s\n' "$a"
    fi
  done < <(era_assets "$tag")
}

# Capture the existence probe's stderr, so a 404 can be told apart from any
# other API failure: 404 means "no release" (dispatch path), anything else is
# unknown state and must fail closed rather than dispatch on an error (guessing
# "missing" here is the spurious-dispatch class fixed above).
API_ERR="$(mktemp)"
trap 'rm -f "$API_ERR"' EXIT

release_id=""
if release_id="$(gh api "repos/${GITHUB_REPOSITORY}/releases/tags/${TAG}" \
      --jq '.id' 2>"$API_ERR")"; then
  if [[ -z "$release_id" || "$release_id" == "null" ]]; then
    echo "::error::release $TAG resolved without an id - refusing to guess whether the release exists (webfang#1535)." >&2
    exit 1
  fi
  # The release exists. Read its assets BY ID — never from the by-tag object,
  # whose embedded .assets were measured empty while this endpoint returned 5.
  # The response is consumed as raw JSON and parsed with jq in two passes: one
  # for presence, one for sizes (audit B8 — an asset present with size 0 is
  # a corrupt upload, not a complete release). A response that jq cannot
  # parse is an unreadable API state, fail-closed like any other.
  assets_json=""
  if ! assets_json="$(gh api "repos/${GITHUB_REPOSITORY}/releases/${release_id}/assets" 2>"$API_ERR")"; then
    echo "::error::release $TAG exists (id $release_id) but its assets could not be read ($(tr '\n' ' ' <"$API_ERR")) - failing closed instead of guessing: calling it 'incomplete' is the spurious dispatch of run 35844323775, calling it 'complete' would hide missing binaries." >&2
    exit 1
  fi
  if ! present_sizes="$(printf '%s' "$assets_json" | jq -e -r 'map("\(.name)\t\(.size)") | join("\n")' 2>"$API_ERR")"; then
    echo "::error::release $TAG exists (id $release_id) but its asset payload is not parseable JSON (jq: $(tr '\n' ' ' <"$API_ERR")) - failing closed instead of guessing whether the inventory is complete." >&2
    exit 1
  fi
  declare -A asset_size=()
  while IFS=$'\t' read -r name size; do
    asset_size["$name"]="$size"
  done <<<"$present_sizes"
  missing=(); empty=()
  while IFS= read -r req; do
    if [[ -z "${asset_size[$req]:-}" ]]; then
      missing+=("$req")
    elif [ "${asset_size[$req]}" -eq 0 ] 2>/dev/null; then
      empty+=("$req")
    fi
  done < <(required_asset_names "$TAG")
  if [[ "${#missing[@]}" -gt 0 ]]; then
    echo "Release $TAG exists but is incomplete; missing: ${missing[*]}"
  elif [[ "${#empty[@]}" -gt 0 ]]; then
    echo "Release $TAG exists but is incomplete; present with size 0: ${empty[*]}"
  else
    echo "Release $TAG is already complete; nothing to do."
    exit 0
  fi
elif grep -q 'HTTP 404' "$API_ERR"; then
  echo "Release $TAG does not exist yet."
else
  echo "::error::could not determine whether release $TAG exists: $(tr '\n' ' ' <"$API_ERR") - failing closed instead of dispatching on an unreadable API response (webfang#1535)." >&2
  exit 1
fi

# The release exists but is not complete for its era, or does not exist at all.
# If the tag predates the rebuildable floor, no dispatch can change that: the
# tree pins a toolchain this pipeline does not install, so the build dies with
# E0463 and the release stays exactly as it is. Dispatching it 33 times a day is
# the noise webfang#2493 removed. Say what it is and leave it alone — the sweep
# counts this line as a permanent gap, never as a dispatch and never as success.
if ! tag_at_or_after "$TAG" "$REBUILDABLE_FROM_TAG"; then
  echo "Release $TAG cannot be rebuilt by the current pipeline: its tree predates $REBUILDABLE_FROM_TAG and pins its own toolchain, so this is a permanent gap, not a pending dispatch. Backfill it by hand if those artifacts are needed."
  exit 0
fi

# Bounded retry with backoff. Three attempts covers a transient API failure
# without turning a real outage into an unbounded loop.
ATTEMPTS=3
RELEASE_DISPATCH_BACKOFF_SECONDS="${RELEASE_DISPATCH_BACKOFF_SECONDS:-10}"
for (( attempt = 1; attempt <= ATTEMPTS; attempt++ )); do
  # Build the dispatch command with optional expected_sha
  # Format matches the extraction regex in check_release_dispatch.sh:
  # backslash-continued lines ending with ; OR ${DISPATCH_CMD[@]} form
  DISPATCH_CMD=(
    gh workflow run release.yml
    --repo "$GITHUB_REPOSITORY"
    -f tag="$TAG"
  )
  if [[ -n "$EXPECTED_SHA" ]]; then
    DISPATCH_CMD+=(-f expected_sha="$EXPECTED_SHA")
  fi

  if "${DISPATCH_CMD[@]}"; then
    echo "Dispatched release.yml for $TAG (attempt $attempt)${EXPECTED_SHA:+ with expected_sha=$EXPECTED_SHA}."
    exit 0
  fi
  echo "::warning::dispatch attempt $attempt/$ATTEMPTS for $TAG failed." >&2
  if (( attempt < ATTEMPTS )); then
    # Overridable so the semantics test does not spend real time waiting;
    # production keeps a real pause between attempts.
    sleep $(( attempt * RELEASE_DISPATCH_BACKOFF_SECONDS ))
  fi
done

echo "::error::could not dispatch release.yml for $TAG after $ATTEMPTS attempts - that tag now has no binaries. Recover with: gh workflow run release.yml --repo $GITHUB_REPOSITORY -f tag=$TAG${EXPECTED_SHA:+ -f expected_sha=$EXPECTED_SHA}" >&2
exit 1