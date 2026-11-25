#!/usr/bin/env bash
# Hermetic string/args alignment harness for the canonical check gate.
#
# Compares the commands declared in odd/gates/canonical-check-spec.md against
# their mirrors in justfile, .pre-commit-config.yaml and
# .github/workflows/ci.yml. It NEVER runs cargo, just, pre-commit or rustdoc:
# only text is read and normalized.
#
# Scope of the CI comparison (deliberate): only three commands are literally
# verifiable there — fmt, clippy with both -W lints, and doc + RUSTDOCFLAGS.
# `cargo check` is a local mirror of the spec: CI invokes `-p/--features`
# variants (e.g. `cargo check -p webfang_cli --features console`), so claiming
# literal equality against ci.yml would be false. That gap is documented here,
# not papered over by an assertion.
#
# RED/GREEN evidence: the unmutated inputs must pass every assertion; each
# NEGATION copies the inputs into $WORK, mutates exactly one file and requires
# the corresponding named assertion to fail.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# Deterministic normalization: drop CR, turn YAML/bare quotes, brackets and
# commas into spaces, collapse whitespace. Cargo subcommands and flags stay
# byte-identical, including -D vs -A and both -W lint names.
normalize_stream() {
  sed \
    -e 's/\r//g' \
    -e 's/"/ /g' \
    -e "s/'/ /g" \
    -e 's/\[/ /g' \
    -e 's/\]/ /g' \
    -e 's/,/ /g' \
    -e 's/[[:space:]]\{1,\}/ /g' \
    -e 's/^ //' \
    -e 's/ $//'
}

# The canonical sequence, in the order declared by the spec.
CANON=(
  'cargo check'
  'cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines'
  'cargo fmt --all -- --check'
  'env RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps'
)

INPUTS=(
  'odd/gates/canonical-check-spec.md'
  'justfile'
  '.pre-commit-config.yaml'
  '.github/workflows/ci.yml'
)

check_inputs() {
  local root="$1" rel f rc=0
  for rel in "${INPUTS[@]}"; do
    f="$root/$rel"
    if [[ ! -f "$f" ]]; then
      printf 'missing input file: %s\n' "$f" >&2
      rc=2
    fi
  done
  return "$rc"
}

# Numbered, backticked commands from the spec ("1. `...`"), normalized.
spec_commands() {
  # \1 is a sed backreference, not a shell expansion (SC2016 false positive).
  # shellcheck disable=SC2016
  sed -nE 's/^[[:space:]]*[0-9]+\.[[:space:]]+`([^`]+)`.*/\1/p' "$1" \
    | normalize_stream \
    | sed '/^$/d'
}

# Command lines of a just recipe. A header resets the scope; @echo, comments
# and blank lines are dropped. canon_from_just then keeps only lines starting
# with a canonical gate prefix, which ignores tests/codedb/continuation lines.
recipe_commands() {
  awk -v name="$2" '
    /^[^[:space:]#][^:]*:/ { active = ($0 ~ ("^" name ":")); next }
    active && /^[[:space:]]/ {
      line = $0
      sub(/^[[:space:]]+/, "", line)
      if (line == "" || line ~ /^#/ || line ~ /^@echo/) next
      print line
    }
  ' "$1"
}

canon_from_just() {
  recipe_commands "$1" "$2" \
    | normalize_stream \
    | sed '/^$/d' \
    | { grep -E '^(cargo check|cargo clippy|cargo fmt|env RUSTDOCFLAGS)' || true; }
}

# Effective command of a local pre-commit hook: `entry` + `args`, with args as
# either an inline YAML list or a block list. YAML list dashes are consumed.
precommit_hook_command() {
  awk -v hook="$2" '
    /^[[:space:]]*- id:[[:space:]]*/ {
      if (active) print entry " " args
      id = $0
      gsub(/\r/, "", id)
      sub(/^[[:space:]]*- id:[[:space:]]*/, "", id)
      active = (id == hook)
      entry = ""; args = ""; inargs = 0
      next
    }
    active && /^[[:space:]]*entry:[[:space:]]*/ {
      line = $0; sub(/^[[:space:]]*entry:[[:space:]]*/, "", line)
      entry = line; inargs = 0; next
    }
    active && /^[[:space:]]*args:[[:space:]]*/ {
      line = $0; sub(/^[[:space:]]*args:[[:space:]]*/, "", line)
      inargs = 1; args = args " " line; next
    }
    active && inargs && /^[[:space:]]*-[[:space:]]*/ {
      line = $0; sub(/^[[:space:]]*-[[:space:]]*/, "", line)
      args = args " " line; next
    }
    active && /^[[:space:]]*[a-z_-]+:/ { inargs = 0; next }
    END { if (active) print entry " " args }
  ' "$1" | normalize_stream | sed '/^$/d'
}

ci_run_commands() {
  # CI steps use both `- run: ...` and `- name: ...` followed by `run: ...`.
  normalize_stream < "$1" | sed -n -e 's/^- run: //p' -e 's/^run: //p'
}

ci_env_flags() {
  normalize_stream < "$1" | grep -E '^RUSTDOCFLAGS:' || true
}

# compare_list <actual-multiline> <expected...>: exact count and order.
compare_list() {
  local actual="$1"; shift
  local n_actual n_expected i=0 want got
  n_actual="$(printf '%s\n' "$actual" | awk 'NF { n++ } END { print n + 0 }')"
  n_expected="$#"
  if [[ "$n_actual" -ne "$n_expected" ]]; then
    return 1
  fi
  for want in "$@"; do
    got="$(printf '%s\n' "$actual" | sed -n "$((i + 1))p")"
    [[ "$got" == "$want" ]] || return 1
    i=$((i + 1))
  done
  return 0
}

verify_all() {
  local root="$1"
  local spec="$root/odd/gates/canonical-check-spec.md" \
    just="$root/justfile" pre="$root/.pre-commit-config.yaml" \
    ci="$root/.github/workflows/ci.yml" f actual runs flags fmt_cmd clippy_cmd \
    before="$FAILED"
  for f in "$spec" "$just" "$pre" "$ci"; do
    if [[ ! -f "$f" ]]; then
      printf '  FAIL missing input: %s\n' "$f"
      return 2
    fi
  done

  actual="$(spec_commands "$spec")"
  if compare_list "$actual" "${CANON[@]}"; then
    pass 'spec numbered commands (count + order)'
  else
    fail 'spec numbered commands (count + order)'
  fi

  actual="$(canon_from_just "$just" check)"
  if compare_list "$actual" "${CANON[@]}"; then
    pass 'just check recipe'
  else
    fail 'just check recipe'
  fi

  actual="$(canon_from_just "$just" test-ci)"
  if compare_list "$actual" "${CANON[@]}"; then
    pass 'just test-ci recipe'
  else
    fail 'just test-ci recipe'
  fi

  # pre-commit covers only 2 of the 4 canonical commands, as the spec declares.
  fmt_cmd="$(precommit_hook_command "$pre" rustfmt-check)"
  clippy_cmd="$(precommit_hook_command "$pre" clippy)"
  if [[ "$fmt_cmd" == "${CANON[2]}" && "$clippy_cmd" == "${CANON[1]}" ]]; then
    pass 'pre-commit fmt/clippy hooks'
  else
    fail 'pre-commit fmt/clippy hooks'
  fi

  # Only three commands are literally verifiable in ci.yml; see header note.
  runs="$(ci_run_commands "$ci")"
  flags="$(ci_env_flags "$ci")"
  if grep -Fqx 'cargo fmt --all -- --check' <<<"$runs" \
    && grep -Fqx "${CANON[1]}" <<<"$runs" \
    && grep -Fqx 'cargo doc --workspace --all-features --no-deps' <<<"$runs" \
    && grep -Fqx 'RUSTDOCFLAGS: -D warnings' <<<"$flags"; then
    pass 'ci fmt/clippy/doc commands'
  else
    fail 'ci fmt/clippy/doc commands'
  fi

  # A failed named assertion must make the whole comparison fail, otherwise a
  # negation would look green just because this function returned success.
  [[ "$FAILED" -eq "$before" ]] || return 1
  return 0
}

new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/odd/gates" "$dir/.github/workflows"
  cp "$REPO_ROOT/odd/gates/canonical-check-spec.md" "$dir/odd/gates/"
  cp "$REPO_ROOT/justfile" "$dir/"
  cp "$REPO_ROOT/.pre-commit-config.yaml" "$dir/"
  cp "$REPO_ROOT/.github/workflows/ci.yml" "$dir/.github/workflows/"
  printf '%s\n' "$dir"
}

# negation <name> <dir> <expected-failing-check> [expected-exit]
negation() {
  local name="$1" dir="$2" expected="$3" want_rc="${4:-1}" out rc=0
  out="$(verify_all "$dir" 2>&1)" || rc=$?
  if [[ "$rc" -eq "$want_rc" ]] && grep -Fq "FAIL $expected" <<<"$out"; then
    pass "RED $name (exit $rc, $expected)"
  else
    fail "$name: expected RED on $expected (exit $rc)"
    printf '%s\n' "$out" | sed -n '1,12p' | sed 's/^/       /'
  fi
}

# Fail closed at the boundary: a missing input is never a green comparison.
check_inputs "$REPO_ROOT" || exit 2

printf 'canonical gate alignment (GREEN: unmutated inputs)\n'
verify_all "$REPO_ROOT"

# NEGATION: drop --all from fmt in the `check` recipe only.
d="$(new_fixture no-all-in-check)"
awk -v recipe='check' '
  /^[^[:space:]#][^:]*:/ { active = ($0 ~ ("^" recipe ":")); print; next }
  active && /cargo fmt --all -- --check/ { sub(/--all /, ""); print; next }
  { print }
' "$d/justfile" >"$d/justfile.new" && mv "$d/justfile.new" "$d/justfile"
negation 'no-all-in-check' "$d" 'just check recipe'

# NEGATION: -D -> -A in the pre-commit clippy hook.
d="$(new_fixture allow-in-precommit)"
sed -i 's/"-D", "warnings"/"-A", "warnings"/' "$d/.pre-commit-config.yaml"
negation 'allow-in-precommit' "$d" 'pre-commit fmt/clippy hooks'

# NEGATION: drop one -W from the `test-ci` clippy line only.
d="$(new_fixture missing-w-in-test-ci)"
awk -v recipe='test-ci' '
  /^[^[:space:]#][^:]*:/ { active = ($0 ~ ("^" recipe ":")); print; next }
  active { gsub(/ -W clippy::too_many_lines/, ""); print; next }
  { print }
' "$d/justfile" >"$d/justfile.new" && mv "$d/justfile.new" "$d/justfile"
negation 'missing-w-in-test-ci' "$d" 'just test-ci recipe'

# NEGATION: invert the first two commands in the `check` recipe.
d="$(new_fixture inverted-check)"
awk -v recipe='check' '
  /^[^[:space:]#][^:]*:/ { active = ($0 ~ ("^" recipe ":")); print; next }
  active && /cargo check/ { hold = $0; next }
  active && hold != "" && /cargo clippy/ { print; print hold; hold = ""; next }
  active && hold != "" { print hold; hold = ""; print; next }
  { print }
' "$d/justfile" >"$d/justfile.new" && mv "$d/justfile.new" "$d/justfile"
negation 'inverted-check' "$d" 'just check recipe'

# NEGATION: remove RUSTDOCFLAGS from the CI doc step.
d="$(new_fixture no-rustdocflags)"
sed -i '/RUSTDOCFLAGS: -D warnings/d' "$d/.github/workflows/ci.yml"
negation 'no-rustdocflags' "$d" 'ci fmt/clippy/doc commands'

# NEGATION: a missing input fails closed with exit 2.
d="$(new_fixture no-justfile)"
rm -f "$d/justfile"
negation 'no-justfile' "$d" 'missing input:' 2

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
(( FAILED == 0 )) || exit 1
