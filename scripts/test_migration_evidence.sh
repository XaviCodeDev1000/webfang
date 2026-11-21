#!/usr/bin/env bash
# Semantics harness for scripts/check_migration_evidence.sh.
#
# Hermetic: builds minimal trees under mktemp, never calls cargo, rustc, the
# network or external services, and never touches the real tree. Two cases
# create a LOCAL git repo with fixed dates to reproduce a SQUASH. Every
# negative case carries its `# NEGATION:`.
# The closed-train boundary case is a POSITIVE CONTROL and therefore carries
# none: the guard must let the drift through when TRAIN_OPEN does not exist.
#
# The meta-guard replaces the guard with `exit 0` and re-runs this harness:
# every negative case passes then, so the harness GOES RED. That is what
# proves the guard decides something and is not decoration.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="${MIGRATION_GUARD:-$ROOT/scripts/check_migration_evidence.sh}"
AUDITOR="$ROOT/scripts/audit_migration_baseline.py"
SHA="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf 'OK: %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$1"; }

fixture() { # -> prints the path of a tree coherent with baseline.json
  local d
  d="$(mktemp -d "$WORK/tree.XXXXXX")"
  mkdir -p "$d/.github/workflows" "$d/evidence/mig-2026-01" \
           "$d/crates/webfang_test_utils" "$d/crates/webfang_core"
  cat > "$d/Cargo.toml" <<'TOML'
[workspace]
resolver = "2"
members = ["crates/webfang_test_utils", "crates/webfang_core"]

[workspace.package]
version = "8.0.2"
edition = "2021"
rust-version = "1.91"

[workspace.lints.rust]
unsafe_code = "deny"
TOML
  printf 'edition.workspace = true\n' > "$d/crates/webfang_test_utils/Cargo.toml"
  printf 'edition.workspace = true\n' > "$d/crates/webfang_core/Cargo.toml"
  cat > "$d/rust-toolchain.toml" <<'TOML'
[toolchain]
channel = "1.91.1"
components = ["rustfmt", "clippy"]
TOML
  cat > "$d/.github/support.json" <<'JSON'
{"schema_version":1,"lines":[
 {"minor":"8.0","state":"STABLE","latest":"8.0.2","accepts":["bugfix","security"]},
 {"minor":"7.7","state":"MAINTENANCE","latest":"7.7.1","accepts":["security"]},
 {"minor":"7.6","state":"EOL","latest":"7.6.6","eol_reason":"auto: rotate-stable, third line back from STABLE"}
]}
JSON
  cat > "$d/README.md" <<'MD'
WebFang is a **binaries-only** distribution. Nothing is published to crates.io.
**Miri status:** the Miri lanes are **advisory**: scheduled-only, continue-on-error, and never a merge gate.
MD
  cat > "$d/.github/workflows/ci.yml" <<'YML'
jobs:
  miri:
    continue-on-error: true
  miri-infra-fast: {}
  miri-infra-slow: {}
YML
  cat > "$d/.github/workflows/release.yml" <<'YML'
matrix:
  include:
    - target: x86_64-unknown-linux-gnu
YML
  python3 "$AUDITOR" --root "$d" --head-sha "$SHA" \
    --write "$d/evidence/mig-2026-01/baseline.json" >/dev/null 2>&1 \
    || { printf 'fixture: --write failed\n' >&2; return 1; }
  cat > "$d/evidence/mig-2026-01/PRD-v2.md" <<'MD'
# PRD fixture
Workspace {{BASELINE.workspace_version}} at {{BASELINE.head_sha}}.
Major {{BASELINE.decisions.major_target}}, backports {{BASELINE.decisions.backport_line}}.
Index: **RF-01** freeze the baseline; **RF-23** semver; **RF-24** backports.
MD
  : > "$d/evidence/mig-2026-01/TRAIN_OPEN"
  printf '%s' "$d"
}

run_guard() { # tree [head_sha]
  local d="$1" sha="${2:-$SHA}"
  MIGRATION_ROOT="$d" \
  MIGRATION_AUDITOR="$AUDITOR" \
  MIGRATION_EVIDENCE="$d/evidence/mig-2026-01/baseline.json" \
  MIGRATION_PRD="$d/evidence/mig-2026-01/PRD-v2.md" \
  MIGRATION_TRAIN_OPEN="$d/evidence/mig-2026-01/TRAIN_OPEN" \
  MIGRATION_HEAD_SHA="$sha" \
    bash "$GUARD" >"$WORK/out.log" 2>&1
  return $?
}

expect_ok()  { local label="$1"; shift; if run_guard "$@"; then ok "$label"; else bad "$label (expected green)"; sed 's/^/    /' "$WORK/out.log"; fi; }
expect_red() { local label="$1"; shift; if run_guard "$@"; then bad "$label (expected red, guard green)"; sed 's/^/    /' "$WORK/out.log"; else ok "$label"; fi; }

# Deterministic identity and date: the CI runner has no global user.name/email,
# so EVERY harness commit must carry them explicitly.
git_identity() {
  export GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z"
  export GIT_AUTHOR_NAME="Migration Test" GIT_AUTHOR_EMAIL="migration@example.com"
  export GIT_COMMITTER_NAME="Migration Test" GIT_COMMITTER_EMAIL="migration@example.com"
}

# Prints "<measured_sha> <squash_sha>": base -> PR -> tip with baseline, plus a
# squash sharing the tip's TREE whose parent is the base. The measured SHA is
# thus NOT an ancestor of the squash and the guard must decide by tree, the
# same way this repo's merge queue does (.github/merge-queue.json, SQUASH).
make_squash_tree() { # dir -> "<measured_sha> <squash_sha>"
  local d="$1" base measured tree
  (
    cd "$d" || exit 1
    git_identity
    git init -q
    git add -A && git commit -q -m base
    base="$(git rev-parse HEAD)"
    printf 'release work\n' > notes.txt
    git add -A && git commit -q -m pr-work
    measured="$(git rev-parse HEAD)"
    # This fixture represents the PRE-squash state: the measured commit is not
    # on a durable ref yet, which is exactly what a release produces.
    # --allow-unmerged-anchor says so explicitly; without the flag --write
    # refuses, because freezing such an anchor leaves it orphaned on the next
    # PR.
    python3 "$AUDITOR" --root "$d" --write "$d/evidence/mig-2026-01/baseline.json" \
      --allow-unmerged-anchor >/dev/null 2>&1 || exit 1
    git add -A && git commit -q -m tip
    tree="$(git rev-parse 'HEAD^{tree}')"
    printf '%s %s\n' "$measured" "$(git commit-tree "$tree" -p "$base" -m squash)"
  )
}

expect_red_message() { # label tree head_sha expected_message
  local label="$1" tree="$2" sha="$3" msg="$4"
  if run_guard "$tree" "$sha"; then
    bad "$label (expected red, guard green)"
    sed 's/^/    /' "$WORK/out.log"
  elif grep -Fq "$msg" "$WORK/out.log"; then
    ok "$label"
  else
    bad "$label (red but without the expected message)"
    sed 's/^/    /' "$WORK/out.log"
  fi
}

# --- 1. positive control: a coherent tree must be green ----------------------
D="$(fixture)" || { echo "FAIL: could not build the fixture"; exit 1; }
expect_ok "baseline + PRD + coherent decisions -> green" "$D"

# --- 2. missing evidence ----------------------------------------------------
# NEGATION: a deleted baseline cannot degrade to "no evidence, moving on".
D2="$(fixture)"; rm -f "$D2/evidence/mig-2026-01/baseline.json"
expect_red "missing baseline -> red" "$D2"

# --- 3. corrupt JSON --------------------------------------------------------
# NEGATION: an unreadable baseline is not an empty baseline.
D3="$(fixture)"; printf '{' > "$D3/evidence/mig-2026-01/baseline.json"
expect_red "baseline with invalid JSON -> red" "$D3"

# --- 4. literal fact reintroduced into the prose -----------------------------
# NEGATION: v1 died from copying figures into the document; v2 bans them.
D4="$(fixture)"; printf '\nv7.7.1\n' >> "$D4/evidence/mig-2026-01/PRD-v2.md"
expect_red "v1 literal fact in the PRD -> red" "$D4"

# --- 5. unresolved placeholder ----------------------------------------------
# NEGATION: an invented placeholder must fail, not be printed as-is.
D5="$(fixture)"; printf '\n{{BASELINE.no.such.key.exists}}\n' >> "$D5/evidence/mig-2026-01/PRD-v2.md"
expect_red "orphan placeholder -> red" "$D5"

# --- 6. orphan RF (the v1 §3 -> §6 drift) ------------------------------------
# NEGATION: C14 cited a requirement that did not exist; the index detects it.
D6="$(fixture)"; printf '\nSee RF-99 for path guards.\n' >> "$D6/evidence/mig-2026-01/PRD-v2.md"
expect_red "RF cited and not defined -> red" "$D6"

# --- 7. decision drift after a release ---------------------------------------
# NEGATION: a release that rotates support.json invalidates RF-23/RF-24 silently.
D7="$(fixture)"; sed -i 's/version = "8.0.2"/version = "9.0.0"/' "$D7/Cargo.toml"
expect_red "derived workspace version -> red with the train open" "$D7"

# --- 8. POSITIVE CONTROL: train closed, the drift is a NOTE ------------------
# Without # NEGATION: this case MUST pass. It is the boundary that keeps the
# gate from being noisy.
D8="$(fixture)"; sed -i 's/version = "8.0.2"/version = "9.0.0"/' "$D8/Cargo.toml"
rm -f "$D8/evidence/mig-2026-01/TRAIN_OPEN"
expect_ok "train closed + drift -> non-blocking NOTE" "$D8"

# --- 9. frozen decision no longer derivable from the policy ------------------
# NEGATION: if support.json rotates, major/backport are recomputed; an old JSON
# with hand-pasted decisions must fall.
D9="$(fixture)"; python3 - "$D9/evidence/mig-2026-01/baseline.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["decisions"]["major_target"] = "1.0.0"
json.dump(d, open(p, "w"))
PY
expect_red "decisions not derivable from support.json -> red" "$D9"

# --- 10. baseline measured over another tree ---------------------------------
# NEGATION: the baseline SHA cannot be any 40-hex string.
D10="$(fixture)"
expect_red "head_sha that is neither HEAD nor an ancestor -> red" "$D10" "0000000000000000000000000000000000000000"

# --- 11. POSITIVE CONTROL: the measured tip survives the SQUASH --------------
# Without # NEGATION: the guard MUST pass when the baseline was measured on the
# PR tip and the squash preserved its tree changing only baseline.json.
D11="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D11")"
if git -C "$D11" merge-base --is-ancestor "$measured" "$squash"; then
  bad "invalid squash fixture: the measured SHA IS an ancestor (does not prove tree-equality)"
else
  expect_ok "measured tip + squash of the same tree -> green" "$D11" "$squash"
fi

# --- 12. baseline measured over ANOTHER TREE ---------------------------------
# NEGATION: a squash with a different tree cannot slip through even with a
# real SHA.
D12="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D12")"
(
  cd "$D12" || exit 1
  git_identity
  printf 'another tree\n' > other.txt
  git add -A && git commit -q -m other
)
other="$(git -C "$D12" rev-parse HEAD)"
python3 - "$D12/evidence/mig-2026-01/baseline.json" "$other" <<'PY'
import json, sys
p, sha = sys.argv[1], sys.argv[2]
d = json.load(open(p))
d["head_sha"] = sha
json.dump(d, open(p, "w"))
PY
expect_red_message "SHA of another tree -> red with the tree message" "$D12" "$squash" "the baseline was measured over another tree"

# --- 13. nonexistent SHA with real git ---------------------------------------
# NEGATION: a nonexistent object cannot slip through the tree branch.
D13="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D13")"
python3 - "$D13/evidence/mig-2026-01/baseline.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["head_sha"] = "f" * 40
json.dump(d, open(p, "w"))
PY
expect_red_message "nonexistent head_sha with real git -> red with the message" "$D13" "$squash" "the baseline was measured over another tree"

# --- 14. repo without .git ---------------------------------------------------
# NEGATION: without git neither ancestry nor tree equivalence can be proven.
D14="$(fixture)"
expect_red_message "repo without .git -> red with the tree message" "$D14" "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "the baseline was measured over another tree"

# --- 15. --write refuses to freeze an orphaned anchor ------------------------
# NEGATION: --write froze a release branch tip; the queue squashed it and the
# commit fell out of the default branch's history forever, so the baseline
# passed once and failed on the next PR. The guard must refuse to write that
# anchor unless the operator asks for it explicitly.
D15="$(fixture)"
(
  cd "$D15" || exit 1
  git_identity
  git init -q
  git add -A && git commit -q -m base
  git branch -M main
  printf 'orphan\n' > orphan.txt
  git add -A && git commit -q -m "tip outside the default branch"
  git checkout -q -b release-line 2>/dev/null || true
  printf 'release\n' > rel.txt
  git add -A && git commit -q -m "release branch tip"
) >/dev/null 2>&1
ORPHAN_SHA="$(git -C "$D15" rev-parse HEAD)"
if [ "$ORPHAN_SHA" = "$(git -C "$D15" rev-parse main)" ]; then
  bad "invalid fixture 15: the tip cannot match the default branch"
else
  if python3 "$AUDITOR" --root "$D15" --head-sha "$ORPHAN_SHA" \
       --write "$WORK/orphan.json" >"$WORK/w.log" 2>&1; then
    bad "--write accepted an orphaned anchor (the defect this check exists for)"
    sed 's/^/    /' "$WORK/w.log"
  elif grep -Fq "the next squash will orphan it" "$WORK/w.log"; then
    ok "--write refuses an anchor no durable ref reaches"
  else
    bad "--write failed but without the expected message"
    sed 's/^/    /' "$WORK/w.log"
  fi
  # The explicit escape must keep existing for the pre-squash state.
  if python3 "$AUDITOR" --root "$D15" --head-sha "$ORPHAN_SHA" \
       --write "$WORK/orphan.json" --allow-unmerged-anchor >"$WORK/w2.log" 2>&1; then
    ok "--allow-unmerged-anchor still accepts the pre-squash state"
  else
    bad "--allow-unmerged-anchor did not accept the pre-squash state"
    sed 's/^/    /' "$WORK/w2.log"
  fi
fi

# --- 16. default branch is not main -----------------------------------------
# NEGATION: the durable refs are the default branch, local and remote, detected
# through origin's HEAD symref. A repository whose default branch is `develop`
# must still prove ancestry; treating a missing `main` as "not an ancestor"
# would block every valid --write there.
D16="$(mktemp -d "$WORK/nonmain.XXXXXX")"
(
  cd "$D16" || exit 1
  git_identity
  git init -q -b develop
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
  git init -q --bare "$D16.git"
  git remote add origin "$D16.git"
  git push -q origin develop
  git fetch -q origin
  git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/develop
) >/dev/null 2>&1
NONMAIN_SHA="$(git -C "$D16" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D16" --head-sha "$NONMAIN_SHA" \
     --write "$D16/evidence/baseline.json" >"$WORK/w3.log" 2>&1; then
  ok "--write proves ancestry through origin/HEAD when main does not exist"
else
  bad "--write refused a durable anchor on a non-main default branch"
  sed 's/^/    /' "$WORK/w3.log"
fi

# --- 17. no durable ref exists in the repository ----------------------------
# NEGATION: without origin (and without main) ancestry cannot be proven. The
# refusal must say so, instead of claiming the commit is "not an ancestor" of
# refs that do not exist here. The explicit escape (--allow-unmerged-anchor)
# keeps serving the pre-squash state.
D17="$(mktemp -d "$WORK/norefs.XXXXXX")"
(
  cd "$D17" || exit 1
  git_identity
  git init -q -b develop
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
) >/dev/null 2>&1
NOREFS_SHA="$(git -C "$D17" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D17" --head-sha "$NOREFS_SHA" \
     --write "$D17/evidence/baseline.json" >"$WORK/w4.log" 2>&1; then
  bad "--write accepted an anchor with no durable ref in the repository"
elif grep -Fq "no durable ref exists in this repository" "$WORK/w4.log"; then
  ok "--write reports missing durable refs instead of a fake orphan claim"
else
  bad "--write failed without the no-durable-ref message"
  sed 's/^/    /' "$WORK/w4.log"
fi

# --- 18. --write root is a repository subdirectory --------------------------
# NEGATION: a root without .git at its top is ambiguous — the no-repo fixture
# is legitimate, but the same shape is reachable in production by pointing
# --root at a subdirectory, where the hazard check would be silently skipped.
# The freeze must tell "no repository at all" apart from "a repository I could
# not resolve" and fail closed on the latter.
D18="$(mktemp -d "$WORK/nested.XXXXXX")"
(
  cd "$D18" || exit 1
  git_identity
  git init -q -b main
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
) >/dev/null 2>&1
mkdir -p "$D18/sub/deeper"
NESTED_SHA="$(git -C "$D18" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D18/sub/deeper" --head-sha "$NESTED_SHA" \
     --write "$WORK/nested.json" >"$WORK/w5.log" 2>&1; then
  bad "--write accepted a repository subdirectory root (hazard check skipped)"
elif grep -Fq "is not its top" "$WORK/w5.log"; then
  ok "--write refuses a repository subdirectory root (fail closed)"
else
  bad "--write failed without the subdirectory refusal message"
  sed 's/^/    /' "$WORK/w5.log"
fi

printf '\nmigration evidence harness: %d OK, %d FAIL\n' "$PASS" "$FAIL"
[[ $FAIL -eq 0 ]] || exit 1
exit 0
