#!/usr/bin/env bash
set -uo pipefail
# Miri lane invariants guard (#2235) — the FOUR invariants that keep the UB
# lane a DETECTOR instead of an abort.
#
# The UB lane (`Miri (core)`) must REPORT, not abort the process. A lane that
# aborts leaves no result to act on — the dead end #2235 exists to close. Four
# invariants keep it a detector:
#
#   1. NO unjustified `--skip` in a Miri job. The skip-net of #1423 deleted a
#      whole test tree per entry and, with it, the UB hiding inside that
#      tree. A skip that survives must carry a WRITTEN REASON of its own in
#      the comment block of its job — one shared sentence listing several
#      skips is not a per-entry reason.
#   2. Every `#[cfg_attr(miri, ignore = "...")]` in the workspace carries a
#      non-empty reason, and every Miri gate in the modules this lot owns
#      carries one (a gate nobody can evaluate is not a gate).
#   3. The `Miri (core)` filter still covers the module of the attributed UB
#      (`application::`). The detection stays Miri's: the aliasing model is
#      never disabled globally to make the lane green.
#   4. Every test that REACHES a declared UB surface carries a PER-TEST gate
#      (or lives in a `mod tests` excluded from Miri at the module level), so
#      the UB is attributed to a test instead of aborting the whole lane. The
#      surfaces are the DECLARED table `UB_SURFACE_ENTRIES` below — one entry per
#      (file, marker) that physically reaches an interpreter-hostile primitive:
#      `clean_html` → `lol_html::rewrite_str` (the servo_arc Tree-Borrows UB,
#      tracker servo/stylo#210) and the `keyctl` syscall that builds a real
#      kernel-keyring credential. A test that matches the scrape marker WITHOUT
#      reaching the surface (the non-2xx arm) is listed in the nominal allowlist
#      with the reason and the evidence that keep it ungated: an invariant that
#      holds by accident is not an invariant. A new surface — every scheduled run
#      that went red opened one — is ADDED to the table, never silenced.
#
# This guard is STATIC: pure text over the checked-out tree. No cargo, no
# Miri, no network, no repo state touched. Exit 0 = every invariant holds;
# exit 1 = at least one failed.
#
# The semantics harness that proves this guard CAN fail is the sibling
# `scripts/test_miri_lane_invariants.sh` (the repo's guard/harness pair).
#
# The guard is a PROGRAM, never a library: the harness calls it as a
# subprocess, so a guard that is broken — or replaced by `exit 0` — cannot
# take the harness down with it. A harness that SOURCES its guard inherits
# the guard's exit path, and then the controls that prove the guard can fail
# never run: that is the fail-open shape this pair exists to close (issue
# #2056, `scripts/test_guard_coverage.sh`).
#
# Usage: bash scripts/check_miri_lane_invariants.sh
# Env:   MIRI_LANE_ROOT — point the guard at a fixture tree.
#        MIRI_LANE_CHECK — run ONE invariant, by name
#                (`check_skips`, `check_gate_reasons`, `check_core_filter`,
#                `check_ub_attribution`). The harness isolates one invariant
#                per fixture: a fixture that violates four invariants at
#                once proves nothing about which one detected it.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="${MIRI_LANE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

# Modules this lot owns: the downloader submodules the skip-net used to
# delete wholesale, plus the module of the attributed UB.
OWNED_MODULES=(
  "crates/webfang_core/src/application/scraper_service.rs"
  "crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/obscura_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/chromiumoxide_downloader.rs"
  "crates/webfang_core/src/infrastructure/downloader/resource_governor.rs"
)

# Miri jobs of ci.yml that run the UB detector.
MIRI_JOBS=(miri miri-infra-fast miri-infra-slow)

# The tests that match the scrape-use-case marker WITHOUT reaching the UB
# surface (the non-2xx arm: fetch_html rejects it before the cleaner runs).
# Each entry is
#   <file>~<test name>~<reason it does not reach the surface>~<evidence>
# A test that matches and is neither gated nor listed here is a FAIL: the
# invariant is explicit on purpose, never satisfied by accident.
# A listed test that no longer exists is a FAIL too: a stale exemption is a
# lie nobody can evaluate.
UB_SURFACE_EXEMPT=(
  "crates/webfang_core/src/application/scraper_service.rs~scrape_span_records_error_outcome_with_error_class~the 404 arm returns before clean_html (fetch_html rejects every non-2xx status before the cleaner)~scoped Miri run of 2026-10-22: 2 passed, 0 failed, 1 ignored"
)

# The DECLARED UB surface: one entry per (file, marker) that physically reaches
# an interpreter-hostile primitive. Each entry is
#   <file>~<marker>~<requires a 2xx status: 0|1>~<what the marker reaches>
# <marker> is an awk ERE matched against the body of every test of the file.
# Adding a surface here is MANDATORY when a new scheduled run goes red on one:
# the 2026-11-09 run aborted three of four Miri jobs on three entry points that
# were NOT in the table, and the 2026-11-10 manual run (42921114313) — after
# those were gated and merged — aborted `Miri (core)` on a FOURTH, at 330 tests
# instead of 119. Each one had been invisible because the abort always happened
# earlier in the same filter. Declaring the surface is what turns that into a
# PR-time failure instead of a red week.
#   - `clean_html` → `lol_html::rewrite_str`: the servo_arc `HeaderSlice`
#     Tree-Borrows UB (tracker servo/stylo#210, still open). It lives in the
#     dependency and not in our cleaning logic, so the test that only exercises
#     the contract AROUND it is gated — the aliasing model is never disabled.
#   - `serialize_matches` (`application/extraction.rs`) → `absolutize_urls`
#     (`application/fragment_urls.rs`) → `lol_html::rewrite_str`: the SAME UB
#     through a SECOND, independent call site. Every file that reaches it is
#     declared here with a marker that matches what ITS OWN tests call:
#     `serialize_case` for the table tests, `flush_spool_tail` for the CLI
#     flush tests.
#   - `with_header` (`application/fragment_markdown.rs`) → `lol_html::rewrite_str`:
#     a THIRD independent call site, module-gated like `html_cleaner`. These
#     three are the ONLY production `rewrite_str` call sites in the crate
#     (verified by grep), so this table now covers the lol_html surface whole:
#     no entry point is left hiding behind an earlier abort in the same filter.
#   - `keyctl` (syscall 250): Miri cannot execute the syscall at all. That is a
#     property of the interpreter, so the test that needs a real kernel keyring
#     is gated and the configuration contract it also verifies stays ungated.
#   - `check_js_dependencies_with` and `run_version_probe_with_timeout`
#     (`cli/preflight.rs`) → `check_obscura_binary` → `probe_obscura_version`
#     → `Command::spawn`: a THIRD interpreter limit, not aliasing. The hybrid
#     strategy resolves an `obscura` binary and probes its version, so the
#     spawn reaches `posix_spawnattr_init`, which Miri cannot call on `linux`
#     at all. The manual run 43111425786 aborted `Miri (core)` there on the
#     SEVENTH entry point, after 798 tests. These markers are COARSE: they also
#     match the tests that assert a MISSING binary is rejected (they resolve to
#     `None` and never spawn) and the pure path-resolution tests, because a text
#     marker cannot tell "resolved an executable and spawned it" from "named one
#     that does not exist".
#
#     The gate is therefore a MODULE-level one — `mod tests` of that file is
#     excluded from Miri, the `domain::html_cleaner::tests` shape — and NOT the
#     8 per-test gates a precise solution would use. The blocker is
#     `scripts/check_file_size.sh`: `cli/preflight.rs` is grandfathered at 3274
#     lines and the ratchet is DESCENDING, so it may not grow by a single line.
#     8 gates are 8 lines, and the tests that would have to move into a
#     `#[path]` child to make room are ~570 lines, above the 500-line hard limit
#     of a NEW file. The per-test shape — the honest one, which keeps the ~60
#     other tests of that file under the interpreter — is tracked separately.
#     The markers above stay declared so the surface is documented even when
#     the module gate is what covers it.
#   - `V1_ONE_PROCESSED_URL` (`cli/scrape_flow.rs`) →
#     `RecordStore::load` → `migrate_v1` → `fs::copy`: an EIGHTH, unrelated
#     interpreter limit, not aliasing. A resume-mode state file written in the
#     pre-v2 envelope shape (one `processed_urls`, no `version` key) is routed
#     through the explicit v1→v2 migration, whose contract is backup-BEFORE-
#     anything-else-touches-the-file. On Linux that copy is `copy_file_range`,
#     syscall 326, which Miri cannot execute at all (upstream miri#4183, with
#     PR miri#5041 still waiting on its author). The manual run 43306869538
#     aborted `Miri (core)` there after 798 tests, on
#     `apply_resume_mode_full_skips_previously_scraped_urls` — the same
#     primitive the six `record_store.rs` migration tests already reach, and
#     the same one `checkpoint.rs` and `state_store.rs` reach through
#     `preserve_pre_migration_backup`. Both of the `scrape_flow.rs` consumers
#     carry a per-test gate here, and the fixture itself is hoisted into one
#     `const` because `cli/scrape_flow.rs` sits exactly at its grandfathered
#     line count and the DESCENDING ratchet leaves no room for the two gates
#     alone: the duplicated 6-line write blocks became one line each, which
#     pays for them.
#
# Why `application/extraction.rs` itself is NOT an entry: a marker matches the
# TEXT of a test body, and there the tests that reach the rewrite
# (`test_matching_selector_returns_the_fragment_unwrapped`, …) and the ones that
# return before it (`test_body_selector_is_passthrough`, `test_invalid_selector_
# falls_back`, `test_empty_html_falls_back`) call the same
# `extract_with_selector`. A marker cannot tell them apart, and declaring the
# file would fail the guard on the three deliberately un-gated early-return
# tests. Those are left to the lane, which reports them, and to the module-level
# gate (the `htmd_converter` shape) when a whole module is UB-routed.
UB_SURFACE_ENTRIES=(
  "crates/webfang_core/src/domain/html_cleaner.rs~clean_html~0~clean_html → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/application/scraper_service.rs~scrape_with_config~1~clean_html → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/infrastructure/converter/htmd_converter.rs~HtmdConverter.convert|converter.convert~0~clean_html → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/application/batch/spool_export.rs~convert_lines|convert_spool~0~extract_page_content → clean_html → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/application/selector_serialization_tests.rs~serialize_case~0~serialize_matches → absolutize_urls (application/fragment_urls.rs) → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/cli/batch_flush_tests.rs~flush_spool_tail|convert_spool~0~flush_spool_tail → convert_spool → extract_content → extract_with_selector → serialize_matches → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/application/fragment_markdown.rs~with_header~0~with_header (fragment_markdown.rs) → lol_html::rewrite_str (servo_arc Tree-Borrows UB, servo/stylo#210)"
  "crates/webfang_core/src/domain/keyring_store.rs~store.build|default_store.build~0~keyctl (syscall 250) building a real kernel-keyring credential"
  "crates/webfang_core/src/cli/preflight.rs~check_js_dependencies_with~0~check_obscura_binary → probe_obscura_version → Command::spawn (posix_spawnattr_init unsupported by Miri, #775)"
  "crates/webfang_core/src/cli/preflight.rs~run_version_probe_with_timeout~0~run_version_probe_with_timeout → Command::spawn (posix_spawnattr_init unsupported by Miri, #775)"
  "crates/webfang_core/src/cli/scrape_flow.rs~V1_ONE_PROCESSED_URL~0~RecordStore::load → migrate_v1 → fs::copy → copy_file_range (syscall 326), unsupported by Miri (upstream miri#4183)"
)

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

# miri_gates <file> — one record per `#[cfg_attr(miri, ignore …)]` gate:
#   <start_line>|<end_line>|<string_reason>|<comment_reason>|<string_form>
# <string_reason> is 1 when the gate carries a non-empty `ignore = "…"`;
# <comment_reason> is 1 when the line that CLOSES the gate carries a `//`
# reason (the `#[cfg_attr(miri, ignore)] // reason` shape).
# Comment lines are skipped: a doc comment that QUOTES an attribute is prose,
# not a gate.
miri_gates() {
  awk '
    function flush(   txt, cmt) {
      if (!in_gate) return
      txt = buf
      cmt = has_comment
      buf = ""
      in_gate = 0
      has_comment = 0
      # Only a Miri gate is the business of this guard; any other cfg_attr is
      # ignored. Both shapes count — one line, or the multi-line form rustfmt
      # writes when the reason is long — because a reason that only exists in
      # the multi-line form must still be verifiable.
      if (txt !~ /cfg_attr\([[:space:]]*miri/) return
      has_str = (txt ~ /ignore[[:space:]]*=[[:space:]]*"[^"]+"/) ? 1 : 0
      has_form = (txt ~ /ignore[[:space:]]*=[[:space:]]*"/) ? 1 : 0
      printf "%d|%d|%d|%d|%d\n", start_line, end_line, has_str, cmt, has_form
    }
    {
      line = $0
      if (line ~ /^[[:space:]]*(\/\/|\/\*|\*)/) next
      scrub = line
      gsub(/"[^"]*"/, "\"\"", scrub)
      opens = gsub(/\[/, "", scrub)
      closes = gsub(/\]/, "", scrub)
      if (in_gate) {
        buf = buf "\n" line
        end_line = NR
        depth = depth + opens - closes
        if (depth <= 0) {
          has_comment = (line ~ /\/\//) ? 1 : 0
          flush()
        }
        next
      }
      if (line ~ /cfg_attr\(/) {
        in_gate = 1
        buf = line
        start_line = NR
        end_line = NR
        depth = opens - closes
        if (depth <= 0) {
          has_comment = (line ~ /\/\//) ? 1 : 0
          flush()
        }
        next
      }
    }
    END { flush() }
  ' "$1"
}

# job_block <ci.yml> <job> — the lines of one Miri job (header excluded).
job_block() {
  awk -v job="$2" '
    $0 ~ ("^  " job ":") { in_job = 1; next }
    in_job && $0 ~ "^  [A-Za-z0-9_-]+:" { exit }
    in_job { print }
  ' "$1"
}

# job_comments <ci.yml> <job> — the contiguous comment lines directly above
# the job header: the design block that documents that job.
job_comments() {
  awk -v job="$2" '
    { lines[NR] = $0 }
    $0 ~ ("^  " job ":") {
      for (i = NR - 1; i >= 1; i--) {
        if (lines[i] ~ /^[[:space:]]*#/) print lines[i]
        else break
      }
      exit
    }
  ' "$1"
}

# ---------------------------------------------------------------------------
# Check 1 — no unjustified --skip in a Miri job
# ---------------------------------------------------------------------------

check_skips() {
  local root="$1"
  local ci="$root/.github/workflows/ci.yml"
  local rc=0
  local job block comments names name line other

  if [ ! -f "$ci" ]; then
    printf 'FAIL: check_skips: workflow not found: %s\n' "$ci"
    return 1
  fi

  for job in "${MIRI_JOBS[@]}"; do
    block="$(job_block "$ci" "$job")"
    if [ -z "$block" ]; then
      printf 'FAIL: check_skips: Miri job not found in ci.yml: %s\n' "$job"
      rc=1
      continue
    fi
    comments="$(job_comments "$ci" "$job")"
    # Comment lines are excluded: a future author writing "--skip foo" in a
    # comment must not trip a confusing failure (the block of one job holds
    # the design comments of the next one).
    names="$(printf '%s\n' "$block" | grep -v -E '^[[:space:]]*#' | grep -o -- '--skip [A-Za-z0-9_:-][A-Za-z0-9_:-]*' | awk '{print $2}' | sort -u)"

    for name in $names; do
      # A justification must be a line of its OWN: it names this skip, gives
      # a reason, and does not bundle another skip into the same sentence
      # (one shared sentence is not a per-entry reason).
      line=""
      while IFS= read -r candidate; do
        [ -n "$candidate" ] || continue
        bundled=0
        for other in $names; do
          [ "$other" = "$name" ] && continue
          case "$candidate" in
            *"$other"*) bundled=1 ;;
          esac
        done
        [ "$bundled" -eq 1 ] && continue
        if printf '%s' "$candidate" | grep -Eqi 'motivo|reason|because|exclud|justif|gate'; then
          line="$candidate"
          break
        fi
      done < <(printf '%s\n' "$comments" | grep -F -- "$name")
      if [ -z "$line" ]; then
        printf 'FAIL: check_skips: Miri job %s skips "%s" with no written reason of its own in its comment block\n' "$job" "$name"
        rc=1
      else
        printf 'OK: check_skips: Miri job %s justifies --skip %s\n' "$job" "$name"
      fi
    done
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_skips: no unjustified --skip in any Miri job\n'
  fi
  return "$rc"
}

# ---------------------------------------------------------------------------
# Check 2 — every Miri gate carries a written reason
# ---------------------------------------------------------------------------

check_gate_reasons() {
  local root="$1"
  local rc=0
  local file owned start end has_str has_comment has_form

  # 2a. Workspace-wide: a gate written as `ignore = "…"` must carry a
  #     non-empty reason. A gate without a reason is a gate nobody can
  #     evaluate.
  while IFS= read -r file; do
    while IFS='|' read -r start end has_str has_comment has_form; do
      [ -n "$start" ] || continue
      [ "$has_form" = "1" ] || continue
      [ "$has_str" = "1" ] && continue
      printf 'FAIL: check_gate_reasons: %s: Miri gate at line %s has an empty reason\n' "$file" "$start"
      rc=1
    done < <(miri_gates "$file")
  done < <(find "$root/crates" -name '*.rs' -type f | sort)

  # 2b. Owned modules: EVERY Miri gate carries a reason, in the
  #     `ignore = "…"` form or as a same-line `//` reason.
  for owned in "${OWNED_MODULES[@]}"; do
    file="$root/$owned"
    [ -f "$file" ] || continue
    while IFS='|' read -r start end has_str has_comment has_form; do
      [ -n "$start" ] || continue
      if [ "$has_str" != "1" ] && [ "$has_comment" != "1" ]; then
        printf 'FAIL: check_gate_reasons: %s: Miri gate at line %s carries no written reason\n' "$owned" "$start"
        rc=1
      fi
    done < <(miri_gates "$file")
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_gate_reasons: every Miri gate carries a written reason\n'
  fi
  return "$rc"
}

# ---------------------------------------------------------------------------
# Check 3 — the Miri (core) filter still covers the module of the UB
# ---------------------------------------------------------------------------

check_core_filter() {
  local root="$1"
  local ci="$root/.github/workflows/ci.yml"
  local filter

  if [ ! -f "$ci" ]; then
    printf 'FAIL: check_core_filter: workflow not found: %s\n' "$ci"
    return 1
  fi

  filter="$(job_block "$ci" miri | awk '
    /^[[:space:]]*- group: core/ { in_core = 1; next }
    in_core && /filter:/ { print; in_core = 0 }
  ')"

  if [ -z "$filter" ]; then
    printf 'FAIL: check_core_filter: the miri job has no filter for the core group\n'
    return 1
  fi
  if ! printf '%s' "$filter" | grep -q 'application::'; then
    printf 'FAIL: check_core_filter: the Miri (core) filter does not cover application:: — the module of the attributed UB would not be swept\n'
    return 1
  fi
  printf 'OK: check_core_filter: the Miri (core) filter covers application::\n'
  return 0
}

# ---------------------------------------------------------------------------
# Check 4 — the UB is attributed to a test, not to the whole lane
# ---------------------------------------------------------------------------

# test_gates <file> — one record per test function of the file:
#   <name>|<drives_scrape>|<reaches_ub_surface>|<has_gate_with_reason>
# A test "drives the scrape" when its body calls `scrape_with_config(`; it
# "reaches the UB surface" when it does so with a 2xx response — the only
# path that gets to `clean_html` → `lol_html::rewrite_str` (fetch_html
# rejects every non-2xx status before the cleaner runs).
# test_gates <file> <marker> <requires a 2xx status: 0|1> — one record per test
# function of the file: <name>|<matches marker>|<reaches the surface>|<has gate>.
# A test "reaches the surface" when it matches <marker> and, when the entry
# demands it, its body builds a 2xx response — the only path that gets to
# `clean_html` → `lol_html::rewrite_str` (fetch_html rejects every non-2xx
# status before the cleaner runs).
test_gates() {
  awk -v marker="$2" -v need_status="$3" '
    function brackets(text,   scrub, n, m) {
      scrub = text
      gsub(/"[^"]*"/, "\"\"", scrub)
      n = gsub(/\[/, "", scrub)
      m = gsub(/\]/, "", scrub)
      return n - m
    }
    function gate_in(text) {
      if (text !~ /cfg_attr\([[:space:]]*miri/) return 0
      if (text ~ /ignore[[:space:]]*=[[:space:]]*"[^"]+"/) return 1
      # A same-line `//` reason. The regex must tolerate the SPACE after `//`:
      # every real reason is `// text`, so requiring a non-space immediately
      # after the slashes made the dominant gate shape invisible here.
      if (text ~ /\/\/[[:space:]]*[^[:space:]]/) return 1
      return 0
    }
    function flush_fn(   matched, reaches) {
      if (fn_name == "") return
      matched = (body ~ marker) ? 1 : 0
      reaches = (matched && (need_status != "1" || body ~ /status:[[:space:]]*2[0-9][0-9]/)) ? 1 : 0
      printf "%s|%d|%d|%d\n", fn_name, matched, reaches, fn_gate
      fn_name = ""
      body = ""
      fn_gate = 0
    }
    {
      line = $0
      if (attr_depth > 0) {
        attrs = attrs "\n" line
        attr_depth = attr_depth + brackets(line)
        if (attr_depth <= 0) pending_gate = gate_in(attrs)
        next
      }
      if (line ~ /^[[:space:]]*#\[/) {
        attrs = attrs "\n" line
        attr_depth = brackets(line)
        if (attr_depth <= 0) pending_gate = gate_in(attrs)
        next
      }
      if (line ~ /^[[:space:]]*(\/\/|\/\*|\*)/) next
      if (line ~ /^[[:space:]]*(pub[[:space:]]+)?(async[[:space:]]+)?fn[[:space:]]+[A-Za-z0-9_]+/) {
        flush_fn()
        match(line, /fn[[:space:]]+[A-Za-z0-9_]+/)
        fn_name = substr(line, RSTART + 3, RLENGTH - 3)
        is_test = (attrs ~ /#\[test\]/) || (attrs ~ /#\[tokio::test\]/)
        if (!is_test) fn_name = ""
        fn_gate = pending_gate
        pending_gate = 0
        # A one-line body (`fn x() { … }`) keeps its code on the fn line.
        # Discarding that tail makes the marker miss such a test entirely, and
        # the invariant would inspect nothing while reporting green.
        if (fn_name != "") {
          rest = substr(line, RSTART + RLENGTH)
          body = (rest ~ /[^[:space:]]/) ? rest : ""
        } else {
          body = ""
        }
        attrs = ""
        attr_depth = 0
        next
      }
      if (line ~ /^[[:space:]]*$/) next
      attrs = ""
      attr_depth = 0
      pending_gate = 0
      if (fn_name != "") body = body "\n" line
    }
    END { flush_fn() }
  ' "$1"
}

# module_gated <file> — 1 when the file's `mod tests` is excluded from Miri
# builds at the module level (`#[cfg(all(test, not(miri)))]`). That shape is a
# gate too — the repo's own precedent for a module whose every test routes
# through the UB surface (`domain::html_cleaner::tests`) — and must not be
# mistaken for a missing one.
module_gated() {
  awk '
    /^[[:space:]]*#\[/ { attr = attr $0 "\n"; next }
    /^[[:space:]]*(pub[[:space:]]+)?mod[[:space:]]+tests/ {
      if (attr ~ /not\([[:space:]]*miri[[:space:]]*\)/) found = 1
      attr = ""
      next
    }
    { attr = "" }
    END { print (found == 1) ? 1 : 0 }
  ' "$1"
}

# in_exempt_list <file> <test> — 0 when an exemption entry names exactly that
# file/test pair. Matched with awk on the `~` separator, never with a regex
# over the path: a `.` in a path is a literal, not a wildcard.
in_exempt_list() {
  printf '%s\n' "${UB_SURFACE_EXEMPT[@]:-}" |
    awk -v f="$1" -v n="$2" -F'~' '$1 == f && $2 == n { found = 1 } END { exit found ? 0 : 1 }'
}

check_ub_attribution() {
  local root="$1"
  local rc=0
  local entry file marker need_status surface
  local name matched reaches gated module_gate marker_hits
  local exempt_file exempt_name exempt_reason exempt_evidence
  local -a test_names=()
  local all_tests=""

  for entry in "${UB_SURFACE_ENTRIES[@]}"; do
    file="${entry%%~*}"
    marker="$(printf '%s' "$entry" | cut -d'~' -f2)"
    need_status="$(printf '%s' "$entry" | cut -d'~' -f3)"
    surface="$(printf '%s' "$entry" | cut -d'~' -f4-)"
    if [ ! -f "$root/$file" ]; then
      printf 'FAIL: check_ub_attribution: declared UB surface not found: %s\n' "$file"
      rc=1
      continue
    fi
    module_gate="$(module_gated "$root/$file")"
    marker_hits=0
    test_names=()
    while IFS='|' read -r name matched reaches gated; do
      [ -n "$name" ] || continue
      test_names+=("$name")
      all_tests+="$file~$name"$'\n'
      [ "$matched" = "1" ] && marker_hits=$((marker_hits + 1))
      # A module-level gate covers every test of the module: there is nothing
      # per-test to demand there.
      [ "$module_gate" = "1" ] && continue
      if [ "$reaches" = "1" ] && [ "$gated" != "1" ]; then
        printf 'FAIL: check_ub_attribution: %s reaches the UB surface (%s) without a per-test Miri gate — the UB would abort the whole lane instead of being attributed\n' "$name" "$surface"
        rc=1
        continue
      fi
      if [ "$need_status" = "1" ] && [ "$matched" = "1" ] && [ "$reaches" != "1" ] && [ "$gated" != "1" ]; then
        # Matches the marker without reaching the surface: it needs a NOMINAL
        # exemption, with the reason and the evidence.
        if in_exempt_list "$file" "$name"; then
          printf 'OK: check_ub_attribution: %s is nominally exempt (matches the marker without reaching the surface)\n' "$name"
        else
          printf 'FAIL: check_ub_attribution: %s matches %s without a per-test Miri gate and without a nominal exemption — the exemption list is the only way to leave it ungated\n' "$name" "$marker"
          rc=1
        fi
      fi
    done < <(test_gates "$root/$file" "$marker" "$need_status")

    if [ "$module_gate" = "1" ]; then
      printf 'OK: check_ub_attribution: %s is gated at the module level\n' "$file"
      continue
    fi

    # Vacuity guard: the marker must still match SOMETHING. A rename of the use
    # case would leave the invariant green while it inspects nothing.
    if [ "$marker_hits" -eq 0 ]; then
      printf 'FAIL: check_ub_attribution: the marker %s matches no test in %s — the marker disappeared and the invariant is not looking at anything\n' "$marker" "$file"
      rc=1
    fi
  done

  # A stale exemption is a lie nobody can evaluate: the test it names must
  # still exist in the file it names.
  for entry in "${UB_SURFACE_EXEMPT[@]:-}"; do
    [ -n "$entry" ] || continue
    exempt_file="${entry%%~*}"
    exempt_name="$(printf '%s' "$entry" | cut -d'~' -f2)"
    exempt_reason="$(printf '%s' "$entry" | cut -d'~' -f3)"
    exempt_evidence="$(printf '%s' "$entry" | cut -d'~' -f4-)"
    if [ -z "$exempt_reason" ] || [ -z "$exempt_evidence" ]; then
      printf 'FAIL: check_ub_attribution: the exemption of %s carries no reason or no evidence\n' "$exempt_name"
      rc=1
    fi
    if ! printf '%s' "$all_tests" | awk -v f="$exempt_file" -v n="$exempt_name" -F'~' '$1 == f && $2 == n { found = 1 } END { exit found ? 0 : 1 }'; then
      printf 'FAIL: check_ub_attribution: stale exemption: %s no longer exists in %s — prune the entry\n' "$exempt_name" "$exempt_file"
      rc=1
    fi
  done

  if [ "$rc" -eq 0 ]; then
    printf 'OK: check_ub_attribution: every test that reaches a declared UB surface is gated\n'
  fi
  return "$rc"
}


# run_checks <root> — every invariant. The guard's own entry point.
run_checks() {
  local root="$1"
  local rc=0
  check_skips "$root" || rc=1
  check_gate_reasons "$root" || rc=1
  check_core_filter "$root" || rc=1
  check_ub_attribution "$root" || rc=1
  return "$rc"
}

# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

printf '== Miri lane invariants (%s)\n' "$REPO_ROOT"
real_rc=0
if [ -n "${MIRI_LANE_CHECK:-}" ]; then
  case "$MIRI_LANE_CHECK" in
    check_skips|check_gate_reasons|check_core_filter|check_ub_attribution)
      "$MIRI_LANE_CHECK" "$REPO_ROOT" || real_rc=1
      ;;
    *)
      printf 'FAIL: unknown MIRI_LANE_CHECK: %s\n' "$MIRI_LANE_CHECK"
      real_rc=1
      ;;
  esac
else
  run_checks "$REPO_ROOT" || real_rc=1
fi
if [ "$real_rc" -eq 0 ]; then
  printf 'OK: all Miri lane invariants hold\n'
else
  printf 'FAIL: at least one Miri lane invariant is broken\n'
fi
exit "$real_rc"
