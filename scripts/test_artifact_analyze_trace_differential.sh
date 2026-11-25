#!/usr/bin/env bash
# scripts/test_artifact_analyze_trace_differential.sh — differential golden test.
#
# Compares the Rust `analyze_trace` binary against `scripts/analyze-trace.sh`
# (the Bash/jq cookbook it replaces) for all 13 commands, the goldens, missing
# files, missing arguments and injected trace ids. stdout is compared byte for
# byte; stderr is compared after normalizing jq's version-specific error line;
# exit codes must match exactly.
#
# Run: bash scripts/test_artifact_analyze_trace_differential.sh
# The binary is built on demand (`cargo build -p webfang_benchmark --bin
# analyze_trace`). Locale is pinned to C so GNU sort/uniq collation is stable.
set -uo pipefail

export LC_ALL=C

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRIPT="$ROOT/scripts/analyze-trace.sh"
BIN_DIR="${CARGO_TARGET_DIR:-$ROOT/target}/debug"
BIN="$BIN_DIR/analyze_trace"
GOLDENS="$ROOT/crates/webfang_benchmark/tests/goldens"

if ! command -v jq >/dev/null 2>&1; then
  echo "refusing: jq is required for the differential baseline" >&2
  exit 2
fi

if [ ! -x "$BIN" ]; then
  cargo build --manifest-path "$ROOT/Cargo.toml" -p webfang_benchmark --bin analyze_trace -q || exit 2
fi

SCRATCH="${WEBSITE_TEST_SCRATCH:-$HOME/.cache/tmp/webfang-artifact-tests}"
mkdir -p "$SCRATCH" || exit 2
case "$(stat -f -c %T "$SCRATCH" 2>/dev/null)" in
  tmpfs | ramfs)
    echo "refusing tmpfs scratch: $SCRATCH" >&2
    exit 2
    ;;
esac
TMP="$(mktemp -d "$SCRATCH/analyze-trace-diff.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# Rich hermetic fixture: spans, errors, WAF, progress, pacing, admission,
# percentiles and the crawl summary. All numbers are integers (the shape the
# FileTraceLayer emits and the goldens pin).
cat >"$TMP/full.jsonl" <<'TRACEEOF'
{"message":"crawl progress","fields":{"pages_crawled":1},"trace_id":"tr-001"}
{"level":"DEBUG","message":"link discovered","fields":{"url":"https://corpus.test/articles/two"},"trace_id":"tr-001"}
{"record":"span_close","level":"INFO","target":"webfang_core::application::crawler::crawl_task","span":"fetch_page","span_duration_ms":12,"trace_id":"tr-001"}
{"record":"span_close","level":"INFO","span":"pipeline_stage","span_duration_ms":34,"trace_id":"tr-001"}
{"record":"span_close","level":"INFO","span":"extract_content","span_duration_ms":56,"trace_id":"tr-002"}
{"level":"ERROR","target":"webfang_core::application::crawler::crawl_task","span":"crawl_task","fields":{"url":"https://corpus.test/blocked/page"}}
{"level":"WARN","message":"WAF challenge detected on protected page","fields":{"status":403}}
{"message":"crawl progress","fields":{"pages_crawled":2,"progress_pct":50,"eta_secs":40}}
{"message":"crawl progress","fields":{"pages_crawled":3}}
{"message":"rate limit wait","fields":{"scope":"global","waited_ms":5,"url":"https://corpus.test/slow","outcome":"shed"}}
{"message":"rate limit wait","fields":{"scope":"global","waited_ms":7,"url":"https://corpus.test/fast","outcome":"completed"}}
{"message":"rate limit wait","fields":{"scope":"global","waited_ms":5,"url":"https://corpus.test/slow","outcome":"cancelled"}}
{"message":"rate limit wait","fields":{"scope":"pacer","waited_ms":3}}
{"message":"scrape recorded","fields":{"duration_ms":12}}
{"message":"scrape recorded","fields":{"duration_ms":34}}
{"message":"scrape recorded","fields":{"duration_ms":56}}
{"message":"semaphore acquire","fields":{"operation":"downloader_governor","waited_ms":2,"in_flight":1}}
{"message":"semaphore acquire","fields":{"operation":"ai_pool","waited_ms":5,"in_flight":2}}
{"message":"semaphore acquire","fields":{"operation":"downloader_governor","waited_ms":8,"in_flight":0}}
{"message":"inference pool permit","fields":{"operation":"ai_pool","waited_ms":3,"in_flight":3}}
{"message":"crawl completed","fields":{"total_pages":10,"succeeded":7,"errors":3,"errors_waf":1,"errors_http":1,"errors_timeout":1,"errors_network":0,"errors_rate_limit":0,"errors_extraction":0,"errors_internal":0,"errors_panic":0,"duration_secs":2,"pages_per_sec":5.0}}
TRACEEOF

# Injection fixture from scripts/test_artifact_analyze_trace.sh (audit B28).
cat >"$TMP/trace_ids.jsonl" <<'TRACEEOF'
{"message": "crawl progress", "fields": {"pages_crawled": 1}, "trace_id": "tr-001"}
{"message": "crawl progress", "fields": {"pages_crawled": 2}, "trace_id": "tr-002"}
{"message": "crawl progress", "fields": {"pages_crawled": 3}, "trace_id": "tr-003"}
TRACEEOF

pass=0
fail=0

ok() {
  printf '  PASS  %s\n' "$1"
  pass=$((pass + 1))
}

bad() {
  printf '  FAIL  %s\n' "$1"
  fail=$((fail + 1))
}

# Compare one invocation: stdout byte-exact, exit code exact, stderr with
# jq version-specific error lines normalized.
compare() {
  local label="$1"
  shift
  bash "$SCRIPT" "$@" >"$TMP/bash.out" 2>"$TMP/bash.err"
  local bash_rc=$?
  "$BIN" "$@" >"$TMP/rust.out" 2>"$TMP/rust.err"
  local rust_rc=$?
  sed -E 's/^jq: error.*$/JQ_ERROR/' "$TMP/bash.err" >"$TMP/bash.err.n"
  sed -E 's/^jq: error.*$/JQ_ERROR/' "$TMP/rust.err" >"$TMP/rust.err.n"
  if [ "$bash_rc" -eq "$rust_rc" ] \
    && cmp -s "$TMP/bash.out" "$TMP/rust.out" \
    && cmp -s "$TMP/bash.err.n" "$TMP/rust.err.n"; then
    ok "$label"
  else
    bad "$label (rc bash=$bash_rc rust=$rust_rc)"
    diff -u "$TMP/bash.out" "$TMP/rust.out" | head -40
    diff -u "$TMP/bash.err.n" "$TMP/rust.err.n" | head -20
  fi
}

echo "analyze-trace Rust differential (LC_ALL=C)"

# The 13 commands on the rich fixture.
compare "errors" "$TMP/full.jsonl" errors
compare "slow (default N)" "$TMP/full.jsonl" slow
compare "slow 3" "$TMP/full.jsonl" slow 3
compare "stages" "$TMP/full.jsonl" stages
compare "progress" "$TMP/full.jsonl" progress
compare "summary" "$TMP/full.jsonl" summary
compare "counts" "$TMP/full.jsonl" counts
compare "urls-failed" "$TMP/full.jsonl" urls-failed
compare "trace (plain id)" "$TMP/trace_ids.jsonl" trace tr-002
compare "waf" "$TMP/full.jsonl" waf
compare "pacing" "$TMP/full.jsonl" pacing
compare "pacing-hot (default N)" "$TMP/full.jsonl" pacing-hot
compare "pacing-hot 2" "$TMP/full.jsonl" pacing-hot 2
compare "admission" "$TMP/full.jsonl" admission
compare "percentiles" "$TMP/full.jsonl" percentiles

# Existing goldens: the contract the Rust parser already pins.
for golden in golden_full.jsonl golden_summary_only.jsonl golden_span_close_only.jsonl; do
  for cmd in errors slow stages progress summary counts urls-failed waf pacing pacing-hot admission percentiles; do
    compare "$golden $cmd" "$GOLDENS/$golden" "$cmd"
  done
done

# Edge cases: injected ids (audit B28), missing id/file/args and unknown command.
compare "trace (id with quote)" "$TMP/trace_ids.jsonl" trace 'tr-001"x'
compare "trace (crafted filter)" "$TMP/trace_ids.jsonl" trace 'x" or .message != "x'
compare "trace (missing id)" "$TMP/trace_ids.jsonl" trace ''
compare "errors (missing file)" "$TMP/does-not-exist.jsonl" errors
compare "no arguments"
compare "unknown command" "$TMP/full.jsonl" not-a-command
compare "empty file" /dev/null errors

echo
if [ "$fail" -eq 0 ]; then
  echo "analyze-trace differential: $pass passed, 0 failed"
  exit 0
fi
echo "analyze-trace differential: $pass passed, $fail failed"
exit 1
