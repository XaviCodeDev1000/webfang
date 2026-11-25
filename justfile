# justfile - webfang
# Local manual tasks (outer loop).

# -- Verification --

default: check

check:
    cargo check
    cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
    cargo fmt --all -- --check
    env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps

check-fast:
    cargo check

# Registered false-claim regressions from issue #2995.
check-docs-truth:
    bash scripts/check_claim_manifest.sh
    bash scripts/test_claim_manifest.sh

# -- Tests --

# Refresh the CodeDB index (agents need it to know what changed)
analyze:
    codedb index . || echo "CodeDB was already up to date"

# Development tests: only the affected scope (typically 8-25 seconds)
test-dev:
    @echo "🚀 Running tests for changed code..."
    cargo nextest run \
        --no-fail-fast \
        --profile dev

# Full test suite for agents (final gate)
test:
    @echo "🧪 Running the full test suite..."
    cargo nextest run \
        --no-fail-fast \
        --profile agent

# Tests with a precise filter (when you know exactly which modules changed)
test-filter filter:
    @echo "🎯 Running filtered tests: {{filter}}"
    cargo nextest run \
        --no-fail-fast \
        --profile dev \
        -E '{{filter}}'

test-ai:
    cargo nextest run --profile agent --features ai

# -- Audit --

audit:
    cargo audit
    cargo deny check
    cargo machete

# -- Coverage --

cov:
    cargo llvm-cov --html --output-dir coverage-llvm

# -- Format --

fmt:
    cargo fmt

# -- Build --

build-release:
    cargo build --release

# -- CI --

# -- Maintenance --

fix-typos:
    typos -w

# -- Setup --

setup:
    @echo "Checking tools..."
    @which cargo-nextest || (echo "Missing: cargo binstall cargo-nextest"; exit 1)
    @which just || (echo "Missing: cargo binstall just"; exit 1)
    @which cargo-machete || (echo "Missing: cargo binstall cargo-machete"; exit 1)
    @which cargo-audit || (echo "Missing: cargo binstall cargo-audit"; exit 1)
    @which cargo-deny || (echo "Missing: cargo binstall cargo-deny"; exit 1)
    @which typos || (echo "Missing: cargo binstall typos-cli"; exit 1)
    @which sccache || (echo "Missing: sccache"; exit 1)
    @echo "Setup complete - all tools verified"

# =============================================
# WATCH MODE (the one used most with the agent)
# =============================================

# Automatic development mode: watches changes and runs only the affected tests
watch-dev:
    @echo "👀 Watch mode enabled - smart tests"
    @echo "   (Stop with Ctrl+C)"
    cargo watch --clear \
        --watch . \
        --ignore "target/" \
        --ignore "logs/" \
        --shell "just test-dev-with-impact"

# Optimized version: excludes AI integration from the development loop
test-dev-with-impact:
    @echo "🚀 Running development tests..."
    @echo "🎯 Running optimized tests (excluding AI integration)..."
    cargo nextest run \
        --profile dev \
        --no-fail-fast \
        -E "not test(ai_integration)"

# =============================================
# FUZZING (security - detects vulnerabilities and panics)
# =============================================

# ----------------------------------------------------------------------------
# Fuzz tier policy (#507 — enforced as CLI flags)
#
# cargo-fuzz reads NO config file: libFuzzer options only reach the fuzz
# binary when passed after `--` on `cargo fuzz run`. The `fuzz_flags()`
# helper below IS the tier policy — there is no fuzz.toml.
#
# Any target added to fuzz/Cargo.toml MUST be registered in `fuzz_flags()`
# (each fuzz recipe below) and in the `fuzz-smoke`/`fuzz-full` target lists.
# Touching a fuzzed function without updating them is a CI regression (#514).
#
# | Tier | Scope                                | max_len    | timeout |
# |------|--------------------------------------|------------|---------|
# | 1    | Core HTML pipeline (parse/clean/convert/readability/extract) | 16384 | 10 |
# | 2    | Sitemap XML                          | 32768      | 15      |
# | 3    | Link/URL processing                  | 8192       | 10      |
# | 4    | Security-sensitive (WAF, compression, cookies) | 4096–32768 | 10–15 |
# | 5    | Content processing (post-extraction) | 8192       | 10      |
# | 6    | Asset extraction                     | 8192       | 10      |
#
# HTML targets run at max_len=16384 (HTML can be large).
# Decompression/sitemap run at max_len=32768 + timeout=15 (slower work).
# HTTP-header-like inputs stay small (max_len=4096).
# ----------------------------------------------------------------------------

# Smoke test: runs each target for 5 seconds
fuzz-smoke:
    #!/usr/bin/env bash
    set -u
    fuzz_flags() {
        case "$1" in
            fuzz_parse_html|fuzz_html_cleaner|fuzz_convert_to_markdown|fuzz_readability_parse|fuzz_extract_text|fuzz_extract_links|fuzz_compression_detect)
                echo "-max_len=16384" ;;
            fuzz_url_validation|fuzz_url_normalization|fuzz_waf_detection|fuzz_wikilinks|fuzz_syntax_highlight|fuzz_slug_from_url|fuzz_extract_assets)
                echo "-max_len=8192" ;;
            fuzz_parse_content_disposition)
                echo "-max_len=4096" ;;
            fuzz_parse_sitemap|fuzz_decompression)
                echo "-max_len=32768 -timeout=15" ;;
            *)
                echo "" ;;
        esac
    }
    echo "🔍 Smoke testing all fuzz targets (5s each)..."
    for target in fuzz_parse_html fuzz_html_cleaner fuzz_convert_to_markdown fuzz_readability_parse fuzz_extract_text fuzz_parse_sitemap fuzz_extract_links fuzz_url_validation fuzz_url_normalization fuzz_waf_detection fuzz_compression_detect fuzz_parse_content_disposition fuzz_decompression fuzz_wikilinks fuzz_syntax_highlight fuzz_slug_from_url fuzz_extract_assets; do
        echo "  → $target"
        cargo +nightly fuzz run "$target" -- $(fuzz_flags "$target") -max_total_time=5 2>&1 | tail -1
    done
    echo "✅ All targets smoke-tested"

# Fuzz a specific target for N seconds (default: 60)
fuzz-target target seconds="60":
    #!/usr/bin/env bash
    set -euo pipefail
    fuzz_flags() {
        case "$1" in
            fuzz_parse_html|fuzz_html_cleaner|fuzz_convert_to_markdown|fuzz_readability_parse|fuzz_extract_text|fuzz_extract_links|fuzz_compression_detect)
                echo "-max_len=16384" ;;
            fuzz_url_validation|fuzz_url_normalization|fuzz_waf_detection|fuzz_wikilinks|fuzz_syntax_highlight|fuzz_slug_from_url|fuzz_extract_assets)
                echo "-max_len=8192" ;;
            fuzz_parse_content_disposition)
                echo "-max_len=4096" ;;
            fuzz_parse_sitemap|fuzz_decompression)
                echo "-max_len=32768 -timeout=15" ;;
            *)
                echo "" ;;
        esac
    }
    echo "🎯 Fuzzing {{target}} for {{seconds}}s..."
    cargo +nightly fuzz run "{{target}}" -- $(fuzz_flags "{{target}}") -max_total_time={{seconds}}

# Fuzz all targets for 10 minutes (for nightly CI)
fuzz-full:
    #!/usr/bin/env bash
    set -u
    fuzz_flags() {
        case "$1" in
            fuzz_parse_html|fuzz_html_cleaner|fuzz_convert_to_markdown|fuzz_readability_parse|fuzz_extract_text|fuzz_extract_links|fuzz_compression_detect)
                echo "-max_len=16384" ;;
            fuzz_url_validation|fuzz_url_normalization|fuzz_waf_detection|fuzz_wikilinks|fuzz_syntax_highlight|fuzz_slug_from_url|fuzz_extract_assets)
                echo "-max_len=8192" ;;
            fuzz_parse_content_disposition)
                echo "-max_len=4096" ;;
            fuzz_parse_sitemap|fuzz_decompression)
                echo "-max_len=32768 -timeout=15" ;;
            *)
                echo "" ;;
        esac
    }
    echo "🔬 Running full fuzz suite (10 min per target)..."
    for target in fuzz_parse_html fuzz_html_cleaner fuzz_convert_to_markdown fuzz_readability_parse fuzz_extract_text fuzz_parse_sitemap fuzz_extract_links fuzz_url_validation fuzz_url_normalization fuzz_waf_detection fuzz_compression_detect fuzz_parse_content_disposition fuzz_decompression fuzz_wikilinks fuzz_syntax_highlight fuzz_slug_from_url fuzz_extract_assets; do
        echo "=== $target ==="
        cargo +nightly fuzz run "$target" -- $(fuzz_flags "$target") -max_total_time=600 || echo "FAILED: $target"
    done
    echo "✅ Full fuzz suite complete"

# Coverage report: how well fuzzing covers the code
fuzz-coverage target:
    @echo "📊 Generating coverage for {{target}}..."
    cargo +nightly fuzz coverage {{target}}
    @echo "✅ Coverage report in fuzz/coverage/{{target}}/"

# Clean fuzzing artifacts (corpus, crashes, coverage)
fuzz-clean:
    @echo "🧹 Cleaning fuzz artifacts..."
    rm -rf fuzz/artifacts/*/crash-* fuzz/artifacts/*/oom-* fuzz/artifacts/*/leak-*
    @echo "✅ Fuzz artifacts cleaned"

# =============================================
# CI / FINAL GATE (before commit / PR)
# =============================================

# Full validation: the canonical gate plus the focused test suite
test-ci:
    @echo "Starting the final gate (CI)..."
    @echo "1/5 → cargo check..."
    cargo check
    @echo "2/5 → Clippy (strict)..."
    cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
    @echo "3/5 → Format check (--all)..."
    cargo fmt --all -- --check
    @echo "4/5 → Rustdoc (deny warnings)..."
    env "RUSTDOCFLAGS=-D warnings" cargo doc --workspace --all-features --no-deps
    @echo "5/5 → Full test suite..."
    codedb index . || echo "CodeDB was already up to date"
    cargo nextest run \
        --profile ci \
        --no-fail-fast
    @echo "CI gate passed - ready to commit/push/PR"

# Quick version: tests + strict clippy only (skips fmt and rustdoc)
test-ci-quick:
    @echo "Quick CI (clippy + tests)..."
    cargo clippy --all-targets --all-features -- -D warnings -W clippy::cognitive_complexity -W clippy::too_many_lines
    codedb index . || echo "CodeDB was already up to date"
    cargo nextest run --profile ci --no-fail-fast
    @echo "Quick CI gate passed"

# -- Documentation --

# Path of the combined wiki (Obsidian vault)
wiki_output := home_dir() / "Documentos" / "obsidian" / "webfang" / "wiki.md"

# Downloads the combined documentation (narrative + rustdoc API) generated in CI.
# All heavy work (mdBook build/test/linkcheck + cargo doc + rustdoc-markdown)
# runs in GitHub Actions; locally this only pulls the latest successful main
# run. Requires an authenticated gh CLI (gh auth login).
docs:
    @echo "📚 Downloading documentation generated in CI..."
    @mkdir -p "$(dirname '{{wiki_output}}')"
    @# No pre-guard on `gh auth status`: it queries the keyring and can report
    @# transient false negatives even when gh works; the real failure is named below.
    @RUN_ID=$(gh run list -R XaviCode1000/webfang --workflow docs.yml --branch main --status success --limit 1 --json databaseId --jq '.[0].databaseId') \
    || { echo "⚠️  gh run list failed - check the gh token: gh auth login"; exit 1; }; \
    test -n "$RUN_ID" || { echo "⚠️  No successful docs.yml runs on main"; exit 1; }; \
    echo "   Main run: $RUN_ID"; \
    rm -rf target/docs-llm && \
    gh run download -R XaviCode1000/webfang "$RUN_ID" -n webfang-docs-llm -D target/docs-llm
    @# The artifact may use a directory layout (post-#734) - accept both.
    @FULL=target/docs-llm/webfang-docs-full.md; \
    test -f "$FULL" || FULL=target/docs-llm/webfang-docs-llm/webfang-docs-full.md; \
    test -f "$FULL" || FULL=target/docs-llm/webfang-docs-llm.md; \
    test -f "$FULL" || { echo "⚠️  webfang-docs-full.md not found in the artifact"; exit 1; }; \
    cp "$FULL" "{{wiki_output}}"
    @# Freshness notice: the "Source commit:" header of the monolith (audit B30)
    @# vs the local HEAD. Docs newer than HEAD are the normal case (main is
    @# ahead locally); warn only when they lag behind or diverge.
    @ART_SHA=$(sed -n 's/^Source commit: //p' "{{wiki_output}}" | head -n1); \
    HEAD_SHA=$(git rev-parse HEAD 2>/dev/null); \
    if [ -n "$ART_SHA" ] && [ -n "$HEAD_SHA" ] && [ "$ART_SHA" != "$HEAD_SHA" ]; then \
      if git merge-base --is-ancestor "$ART_SHA" "$HEAD_SHA" >/dev/null 2>&1; then \
        echo "⚠️  Docs are from commit $(printf '%s\n' "$ART_SHA" | cut -c1-7), older than your HEAD ($(printf '%s\n' "$HEAD_SHA" | cut -c1-7)): CI may not have finished for your latest commits."; \
      elif ! git merge-base --is-ancestor "$HEAD_SHA" "$ART_SHA" >/dev/null 2>&1; then \
        echo "⚠️  Docs ($(printf '%s\n' "$ART_SHA" | cut -c1-7)) and your HEAD ($(printf '%s\n' "$HEAD_SHA" | cut -c1-7)) diverged."; \
      fi; \
    fi
    @echo "✅ Documentation updated (from CI, latest successful main run):"
    @ls -lh "{{wiki_output}}" | awk '{print "  " $NF " (" $5 ")"}'

# Lightweight local mdBook preview (no remote): builds and serves only the
# narrative at http://localhost:3000. Does not generate rustdoc or the LLM artifact.
docs-local:
    @echo "🔧 Local mdBook preview (no remote)..."
    @mise install aqua:rust-lang/mdBook
    @mdbook build docs
    @echo "📖 Serving at http://localhost:3000 - Ctrl+C to stop"
    @mdbook serve docs -p 3000

# Regenerate the CLI reference chapter from the binary (manual drift)
cli-doc-regen:
    @scripts/gen-cli-reference.sh
