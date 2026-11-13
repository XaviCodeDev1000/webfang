#!/usr/bin/env bash
set -uo pipefail
# Miri lane invariants semantics harness (#2235) — RED before GREEN.
#
# The guard is `scripts/check_miri_lane_invariants.sh`: the four invariants that
# keep the UB lane a DETECTOR. This harness does three things:
#
#   1. runs the guard over the REAL tree (the invariants must hold), and
#   2. proves the guard CAN fail, by running it over `mktemp` fixtures that
#      reproduce the pre-fix shape (#2235): a skip-net with no reason per
#      entry, a Miri gate with no reason, a sibling test that drives the scrape
#      use case with no gate, and a fixture whose marker disappeared (the
#      vacuity case: an invariant that matches nothing must fail, not pass), and
#   3. proves that a guard that CANNOT fail is caught: the meta-guard
#      `scripts/test_guard_coverage.sh` replaces the guard with `exit 0` and
#      requires this harness to go red. That is why the guard is called as a
#      PROGRAM (a subprocess) and never sourced: a sourced guard's `exit 0`
#      would take this harness down with it, the controls below would never
#      run, and the harness would report green over a guard that decides
#      nothing — the fail-open shape issue #2056 exists to close.
#
# Each invariant is isolated: the fixture that violates it is checked with
# `MIRI_LANE_CHECK` naming that invariant only, so a red proves THAT check
# detected the shape, not that some other check tripped over the fixture.
#
# If the checks stopped detecting those, this harness would be a green light
# wired to nothing. A check whose positive control fails is a broken harness,
# not a finding.
#
# STATIC: pure text plus mktemp fixtures. No cargo, no Miri, no network, no
# repo state touched. Exit 0 = the guard holds AND it can still fail.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GUARD="$SCRIPT_DIR/check_miri_lane_invariants.sh"
REPO_ROOT="${MIRI_LANE_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"

[ -f "$GUARD" ] || { echo "test_miri_lane_invariants: no guard at $GUARD" >&2; exit 1; }

FAILURES=0

# run_guard <root> <check> — the guard's exit code over <root>.
#
# The guard runs as a subprocess with `MIRI_LANE_ROOT` pointing at the tree it
# has to judge and `MIRI_LANE_CHECK` naming the single invariant to run (an
# empty name runs all of them, which is how the real tree is judged).
run_guard() {
  local root="$1" check="$2"
  MIRI_LANE_ROOT="$root" MIRI_LANE_CHECK="$check" bash "$GUARD" >/dev/null 2>&1
}

# expect_rc <what> <want_rc> <got_rc>
expect_rc() {
  local what="$1" want="$2" got="$3"
  if [ "$got" != "$want" ]; then
    printf 'FAIL: %s: expected exit %s, got %s\n' "$what" "$want" "$got"
    FAILURES=$((FAILURES + 1))
    return 0
  fi
  printf 'OK: %s\n' "$what"
}

# write_ub_surfaces <root> — the GREEN shape of every declared UB-surface
# entry: each file whose tests match its marker carries either a module-level
# gate or a per-test gate. `scraper_service.rs` is NOT written here: the caller
# decides its shape, because the pre-fix shape of THAT module is the original
# RED control. Every file below is a real fixture, never a comment: a check that
# only ever sees its red shape is not proven to accept the green one.
write_ub_surfaces() {
  local root="$1"
  mkdir -p "$root/crates/webfang_core/src/application/batch" \
    "$root/crates/webfang_core/src/cli" \
    "$root/crates/webfang_core/src/infrastructure/converter" \
    "$root/crates/webfang_core/src/domain"

  # html_cleaner: every test routes through clean_html — module-level gate.
  cat >"$root/crates/webfang_core/src/domain/html_cleaner.rs" <<'EOF'
#[cfg(all(test, not(miri)))]
mod tests {
    #[test]
    fn clean_removes_scripts_and_preserves_content() {
        let cleaned = clean_html("<p>x</p>");
        assert!(cleaned.contains("x"));
    }
}
EOF

  # htmd_converter: same shape — the pre-pass is part of the adapter contract.
  cat >"$root/crates/webfang_core/src/infrastructure/converter/htmd_converter.rs" <<'EOF'
#[cfg(all(test, not(miri)))]
mod tests {
    #[test]
    fn converts_headings_to_atx() {
        let md = HtmdConverter.convert("<h1>Main Title</h1>");
        assert!(md.contains("# Main Title"), "got: {md}");
    }
}
EOF

  # spool_export: the conversion tests are gated per test, the reader-only ones
  # are not — they never convert a page, so they never reach the cleaner.
  cat >"$root/crates/webfang_core/src/application/batch/spool_export.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn reader_tracks_bytes_and_resumes_at_offset() {
        let _ = valid_line("https://example.test/a", "A");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB through clean_html; the reader never converts a page")]
    async fn convert_spool_counts_corrupt_records_and_reports_watermark() {
        let _ = convert_lines(&[valid_line("https://example.test/a", "A")]).await;
    }
}
EOF

  # keyring_store: the real-keyring build is gated, the configuration test is not.
  cat >"$root/crates/webfang_core/src/domain/keyring_store.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn the_production_store_addresses_the_legacy_key() {
        let store = super::store().expect("store");
        assert_eq!(store.delimiters.len(), 3);
    }

    #[test]
    #[cfg_attr(miri, ignore = "keyctl (syscall 250) building the kernel-keyring Cred is not supported by Miri")]
    fn the_real_keyring_key_is_byte_identical_to_the_legacy_key() {
        let store = super::store().expect("store");
        let _ = store.build(SERVICE, ACCOUNT, None);
    }
}
EOF

  # fragment_markdown: the whole module is the gate unit — same shape as the
  # real file, where a marker cannot tell the early-return cases from the
  # ones that reach lol_html::rewrite_str.
  cat >"$root/crates/webfang_core/src/application/fragment_markdown.rs" <<'EOF'
#[cfg(all(test, not(miri)))]
mod tests {
    #[test]
    fn with_header_adds_the_synthetic_header_row() {
        let out = with_header("<table><tr><td>a</td></tr></table>");
        assert!(out.contains("<th>"));
    }
}
EOF

  # scraper_service: one 2xx scrape WITH a gate, and the non-2xx sibling that
  # matches the marker without reaching the surface — the one the nominal
  # exemption names. Both entries of that pair must exist, or the check
  # cannot distinguish "gated" from "not examined".
  cat >"$root/crates/webfang_core/src/application/scraper_service.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    #[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB through clean_html")]
    fn scrape_of_a_clean_page_records_the_success_outcome() {
        let _ = scrape_with_config(wiremock_response(200));
    }

    #[test]
    fn scrape_span_records_error_outcome_with_error_class() {
        let _ = scrape_with_config(wiremock_response(404));
    }
}
EOF

  # selector_serialization_tests: two gated cases through serialize_case.
  cat >"$root/crates/webfang_core/src/application/selector_serialization_tests.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    #[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB through absolutize_urls")]
    fn selector_serialization_absolutizes_the_fragment_urls() {
        let _ = serialize_case("<a href=\"/rel\">x</a>", "selector");
    }
}
EOF

  # batch_flush_tests: the tail-flush tests are gated, the reader-only one is not.
  cat >"$root/crates/webfang_core/src/cli/batch_flush_tests.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn flush_records_are_read_back_in_order() {
        let _ = read_back(&spool);
    }

    #[test]
    #[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB through convert_spool")]
    fn flush_spool_tail_writes_the_final_line() {
        let _ = flush_spool_tail(&spool);
    }
}
EOF

  # preflight: the WHOLE module is the gate unit (#775) — `Command::spawn`
  # reaches `posix_spawnattr_init`, which Miri cannot call on linux, and no
  # text marker separates the tests that spawn from the ones that only name a
  # missing binary. Both declared markers are still matched by the fixture so
  # the surface stays documented; the module gate is what covers it.
  cat >"$root/crates/webfang_core/src/cli/preflight.rs" <<'EOF'
#[cfg(all(test, not(miri)))] // Command::spawn unsupported by Miri (#775)
mod tests {
    #[test]
    fn hybrid_strategy_probes_the_resolved_obscura_binary() {
        let _ = check_js_dependencies_with(&["true"], false, "", &opts);
    }

    #[test]
    fn version_probe_reads_the_child_output_and_the_exit_status() {
        let _ = run_version_probe_with_timeout(&bin_path, DEADLINE);
    }
}
EOF

  # scrape_flow: the pre-v2 resume fixture. Both tests that write it carry a
  # PER-TEST gate — loading that file routes `RecordStore::load` through
  # `migrate_v1`, whose backup-first step is `copy_file_range`, syscall 326,
  # which Miri cannot execute (upstream miri#4183). In the real file the
  # fixture is one `const` because `cli/scrape_flow.rs` sits exactly at its
  # grandfathered line count and the DESCENDING ratchet leaves no room for
  # the two gates by themselves.
  cat >"$root/crates/webfang_core/src/cli/scrape_flow.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[tokio::test]
    #[cfg_attr(miri, ignore = "copy_file_range(326) unsupported by Miri")]
    async fn apply_resume_mode_skips_previously_scraped_urls() {
        std::fs::write(&state_dir.join("example.com.json"), V1_ONE_PROCESSED_URL).unwrap();
    }
}
EOF
}

# red_surface <name> <file> <what it must reject> <body> — a tree whose ONLY
# violation is <what>. Every other entry keeps its green shape, so a red can
# only come from the entry under test: a control that passes for several
# reasons at once proves nothing.
red_surface() {
  local name="$1" file="$2" what="$3" body="$4"
  local r="$T/red-$name"
  rm -rf "$r"
  write_ub_surfaces "$r"
  mkdir -p "$r/$(dirname "$file")"
  printf '%s\n' "$body" >"$r/$file"
  run_guard "$r" check_ub_attribution
  expect_rc "RED control: check_ub_attribution $what ($file)" 1 "$?"
}

printf '== Miri lane invariants (%s)\n' "$REPO_ROOT"

# ---------------------------------------------------------------------------
# Control 1 — the REAL tree: every invariant holds (exit 0).
# ---------------------------------------------------------------------------

run_guard "$REPO_ROOT" ""
expect_rc "the real tree satisfies every Miri lane invariant" 0 "$?"

# ---------------------------------------------------------------------------
# RED control — the harness must be able to FAIL
#
# The fixtures below reproduce the pre-fix shape (#2235): a skip-net with no
# reason per entry, a Miri gate with no reason, and a sibling test that
# drives the scrape use case with no gate. If the checks stopped detecting
# them, this harness would be a green light wired to nothing.
# ---------------------------------------------------------------------------

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT

FIX="$T/fix"
mkdir -p "$FIX/.github/workflows" \
  "$FIX/crates/webfang_core/src/application" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader"

# The workflow carries the THREE Miri jobs the guard inspects, and only the
# `miri` one carries the skip-net: the other two have nothing to justify, so
# the rejection below can only come from the unjustified skip.
cat >"$FIX/.github/workflows/ci.yml" <<'EOF'
jobs:
  miri:
    name: Miri (core)
    strategy:
      matrix:
        include:
          - group: core
            filter: "-- adapters:: application:: cli::"
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- \
            --skip wreq_downloader --skip obscura_downloader \
            -- adapters:: application::
  miri-infra-fast:
    name: Miri (infra-fast)
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- -- infrastructure::
  miri-infra-slow:
    name: Miri (infra-slow)
    steps:
      - name: Setup & Run Miri
        run: |
          cargo miri test --lib -- -- infrastructure::
EOF

# The pre-fix shape of the module of the attributed UB: the test that
# REACHES the UB surface (a 2xx response) has no gate, and the arm that does
# not reach it is the nominally exempt one.
#
# Every OTHER declared UB surface stays GREEN in this tree, and it is written
# FIRST: the RED file below must override the green fixture of the very same
# path, or the control would inspect the green shape and prove nothing.
write_ub_surfaces "$FIX"

cat >"$FIX/crates/webfang_core/src/application/scraper_service.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn scrape_span_records_success_outcome_and_result_count() {
        let client = StubClient { status: 200 };
        let _ = scrape_with_config();
    }

    #[test]
    fn scrape_span_records_error_outcome_with_error_class() {
        let client = StubClient { status: 404 };
        let _ = scrape_with_config();
    }
}
EOF

cat >"$FIX/crates/webfang_core/src/infrastructure/downloader/wreq_downloader.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[cfg_attr(miri, ignore)]
    #[test]
    fn test_wreq_downloader_creation() {}
}
EOF

cp "$FIX/crates/webfang_core/src/application/scraper_service.rs" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader/obscura_downloader.rs"
cp "$FIX/crates/webfang_core/src/application/scraper_service.rs" \
  "$FIX/crates/webfang_core/src/infrastructure/downloader/chromiumoxide_downloader.rs"

# NEGATION: a Miri job that skips a whole submodule with no written reason of
# its own must be REJECTED. Before the fix (#2235) the skip-net of #1423 was
# accepted, and it deleted a whole test tree per entry — with it, the UB
# hiding inside that tree.
run_guard "$FIX" check_skips
expect_rc "RED control: check_skips rejects the unjustified skip-net" 1 "$?"

# NEGATION: a `#[cfg_attr(miri, ignore)]` gate with no reason must be
# REJECTED. A gate nobody can evaluate is not a gate: it silences the test
# without saying what the silence is for.
run_guard "$FIX" check_gate_reasons
expect_rc "RED control: check_gate_reasons rejects a gate with no reason" 1 "$?"

# NEGATION: a test that drives the scrape use case and reaches the UB surface
# with no per-test gate must be REJECTED. The lane has to REPORT the UB, not
# abort on it: an ungated test takes the whole lane down with it.
run_guard "$FIX" check_ub_attribution
expect_rc "RED control: check_ub_attribution rejects the ungated UB test" 1 "$?"

# Vacuity control: the marker that says "this test drives the scrape use
# case" disappears (a rename of the use case). The invariant must FAIL
# instead of inspecting nothing and reporting green.
VACUITY="$T/vacuity"
mkdir -p "$VACUITY/.github/workflows" \
  "$VACUITY/crates/webfang_core/src/application" \
  "$VACUITY/crates/webfang_core/src/infrastructure/downloader"
cp "$FIX/.github/workflows/ci.yml" "$VACUITY/.github/workflows/ci.yml"
for owned in wreq_downloader obscura_downloader chromiumoxide_downloader resource_governor; do
  printf '// fixture\n' >"$VACUITY/crates/webfang_core/src/infrastructure/downloader/$owned.rs"
done
# The green fixtures are written FIRST, so the renamed-marker file below is
# the one the guard inspects.
write_ub_surfaces "$VACUITY"
cat >"$VACUITY/crates/webfang_core/src/application/scraper_service.rs" <<'EOF'
#[cfg(test)]
mod tests {
    #[test]
    fn scrape_span_records_success_outcome_and_result_count() {
        let _ = renamed_use_case();
    }

    #[test]
    fn scrape_span_records_error_outcome_with_error_class() {
        let _ = renamed_use_case();
    }
}
EOF
run_guard "$VACUITY" check_ub_attribution
expect_rc "RED control: check_ub_attribution rejects a fixture whose marker disappeared" 1 "$?"

# The core filter check must ACCEPT a fixture whose filter covers
# `application::` (it is not part of the RED control: the pre-fix tree
# already carried that filter). A check that rejects everything is not a gate
# either — this is the positive control that keeps the RED controls honest.
run_guard "$FIX" check_core_filter
expect_rc "RED control: check_core_filter accepts a fixture that covers application::" 0 "$?"

# ---------------------------------------------------------------------------
# RED controls for the declared UB-surface table (opened by run 42560983718)
#
# The pre-fix shape above is `scraper_service.rs`. The 2026-11-09 scheduled run
# aborted THREE of four Miri jobs on three entry points the guard did not
# declare — `htmd_converter`, `spool_export` and `keyring_store` — so the lane
# stayed red while the invariant above still reported green. Each control below
# proves ONE entry of `UB_SURFACE_ENTRIES` is really inspected, in both of its
# failure shapes: a test that reaches the surface with no gate, and a
# module-level gate that was REMOVED.
# ---------------------------------------------------------------------------

# spool_export: a conversion test with no gate reaches clean_html.
red_surface "spool" "crates/webfang_core/src/application/batch/spool_export.rs" \
'rejects an unguarded conversion test' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn convert_spool_counts_corrupt_records_and_reports_watermark() {
        let _ = convert_lines(&[valid_line("https://example.test/a", "A")]).await;
    }
}'

# htmd_converter: the module-level gate is gone, so every test is unguarded.
red_surface "converter" "crates/webfang_core/src/infrastructure/converter/htmd_converter.rs" \
'rejects a converter module whose Miri gate was removed' \
'#[cfg(test)]
mod tests {
    #[test]
    fn converts_headings_to_atx() {
        let md = HtmdConverter.convert("<h1>Main Title</h1>");
        assert!(md.contains("# Main Title"), "got: {md}");
    }
}'

# html_cleaner: the same removal on the module the pattern came from.
red_surface "cleaner" "crates/webfang_core/src/domain/html_cleaner.rs" \
'rejects a cleaner module whose Miri gate was removed' \
'#[cfg(test)]
mod tests {
    #[test]
    fn clean_removes_scripts_and_preserves_content() {
        let cleaned = clean_html("<p>x</p>");
        assert!(cleaned.contains("x"));
    }
}'

# keyring_store: an un-gated test that builds a real kernel-keyring credential.
red_surface "keyring" "crates/webfang_core/src/domain/keyring_store.rs" \
'rejects an unguarded real-keyring build' \
'#[cfg(test)]
mod tests {
    #[test]
    fn the_real_keyring_key_is_byte_identical_to_the_legacy_key() {
        let store = super::store().expect("store");
        let _ = store.build(SERVICE, ACCOUNT, None);
    }
}'

# Vacuity, per entry: the marker of spool_export disappears (its conversion
# tests are renamed away). The invariant must FAIL instead of inspecting
# nothing — the same honesty the scrape-use-case marker already had.
red_surface "spool-vacuity" "crates/webfang_core/src/application/batch/spool_export.rs" \
'rejects a file whose conversion marker disappeared' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn reader_tracks_bytes_and_resumes_at_offset() {
        let _ = valid_line("https://example.test/a", "A");
    }
}'

# Stale exemption: the exempt test no longer exists while the marker still
# matches a GATED test — so the only thing that can fail here is the staleness
# check. An exemption is a claim about a test that exists.
red_surface "stale-exemption" "crates/webfang_core/src/application/scraper_service.rs" \
'rejects a stale exemption whose test no longer exists' \
'#[cfg(test)]
mod tests {
    #[test]
    #[cfg_attr(miri, ignore = "servo_arc Tree-Borrows UB (servo/stylo#210)")]
    fn scrape_span_records_success_outcome_and_result_count() {
        let _ = scrape_with_config();
    }
}'

# One-line body: `fn x() { … }` carries its code on the fn line. Dropping
# that tail lets a one-line unguarded test reach the UB surface invisibly —
# the invariant would report green over a lane that still aborts.
red_surface "spool-oneline" "crates/webfang_core/src/application/batch/spool_export.rs" \
'rejects an unguarded one-line conversion test' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn convert_spool_counts_corrupt_records_and_reports_watermark() { let _ = convert_lines(&[]).await; }
}'

# selector_serialization_tests: `serialize_case` is the shared arrange helper into
# `serialize_matches` → `absolutize_urls` (fragment_urls.rs) → rewrite_str.
red_surface "serialize-tables" "crates/webfang_core/src/application/selector_serialization_tests.rs" \
'rejects an unguarded selector-serialization table' \
'use super::*;

fn serialize_case(html: &str, selector: &str) -> Option<String> {
    Some(String::new())
}

#[test]
fn selector_fragment_html_table() {
    let _ = serialize_case("<h3>x</h3>", "h3");
}'

# batch_flush: `flush_spool_tail` is the funnel into convert_spool → extract_content
# → extract_with_selector → serialize_matches → rewrite_str. The tests live in the
# `#[path]` child file, so that is where the surface is declared.
red_surface "batch-flush" "crates/webfang_core/src/cli/batch_flush_tests.rs" \
'rejects an unguarded tail-flush test' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn flush_converts_and_writes_the_full_tail() {
        let _ = flush_spool_tail("p", 0, "o", &opts()).await;
    }
}'

# One-line body, on the second entry point: `fn x() { … }` keeps its code on the
# fn line, and a one-line un-gated test must not slip through.
red_surface "batch-flush-oneline" "crates/webfang_core/src/cli/batch_flush_tests.rs" \
'rejects an unguarded one-line tail-flush test' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn flush_keeps_the_watermark() { let _ = flush_spool_tail("p", 0, "o", &opts()).await; }
}'

# fragment_markdown: `with_header` is the third production `rewrite_str` call
# site. The whole module is gated (the htmd_converter shape), so the RED control
# is the removal of that module gate.
red_surface "fragment-markdown" "crates/webfang_core/src/application/fragment_markdown.rs" \
'rejects a fragment_markdown module whose Miri gate was removed' \
'#[cfg(test)]
mod tests {
    #[test]
    fn issue_2540_column_count_drives_the_header_width() {
        let out = with_header("<table><tr><td>a</td></tr></table>");
        assert!(out.contains("Column 1"), "got: {out}");
    }
}'

# preflight: the seventh entry point, found by the manual run 43111425786
# after 798 tests. TWO markers are declared for this one file —
# `check_js_dependencies_with` (the hybrid strategy that probes a resolved
# obscura binary) and `run_version_probe_with_timeout` (the direct probe) —
# because they are the two doors into the same `Command::spawn`. The gate is
# a MODULE-level one, so the RED control is its removal, and the removed gate
# leaves both markers matched and un-gated: a symmetric counterpart of the
# per-test RED controls above.
red_surface "preflight" "crates/webfang_core/src/cli/preflight.rs" \
'rejects a preflight module whose Miri gate was removed' \
'#[cfg(test)]
mod tests {
    #[test]
    fn hybrid_strategy_ok_without_chromium_feature() {
        let _ = check_js_dependencies_with(&["definitely-not-installed-9x4k"], false, "", &opts);
    }

    #[test]
    fn version_probe_reports_normal_exit_and_output() {
        let _ = run_version_probe_with_timeout(&bin_path, std::time::Duration::from_secs(5));
    }
}'

# scrape_flow: the eighth entry point, found by the manual run 43306869538.
# A resume-mode state file written in the pre-v2 envelope shape (one
# `processed_urls`, no `version` key) is routed through the explicit v1→v2
# migration, whose contract is backup-BEFORE anything else touches the file.
# On Linux that copy is `copy_file_range`, syscall 326, which Miri cannot
# execute (upstream miri#4183) — the same primitive the six `record_store.rs`
# migration tests already reach. Both of the file's consumers are gated.
red_surface "scrape-flow" "crates/webfang_core/src/cli/scrape_flow.rs" \
'rejects an unguarded test that writes the pre-v2 resume fixture' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn apply_resume_mode_skips_previously_scraped_urls() {
        std::fs::write(&state_dir.join("example.com.json"), V1_ONE_PROCESSED_URL).unwrap();
    }
}'

# Vacuity, per entry: `V1_ONE_PROCESSED_URL` disappears — the fixture is
# renamed, or the two resume tests that used it are replaced by a v2 store.
# The invariant must FAIL instead of inspecting nothing and reporting green.
red_surface "scrape-flow-vacuity" "crates/webfang_core/src/cli/scrape_flow.rs" \
'rejects a file whose pre-v2 fixture marker disappeared' \
'#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn apply_resume_mode_skips_previously_scraped_urls() {
        std::fs::write(&state_dir.join("example.com.json"), v2_state()).unwrap();
    }
}'

if [ "$FAILURES" -ne 0 ]; then
  printf 'FAIL: the harness cannot detect the pre-fix shape\n'
  exit 1
fi
printf 'OK: the harness detects the pre-fix shape (RED control)\n'
exit 0
