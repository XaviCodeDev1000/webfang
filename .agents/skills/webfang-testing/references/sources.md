# Sources — webfang-testing

This skill is the actionable layer. The normative testing rules and the six-node diagnostic live
in the repo; the shared harness is the source of truth for the helpers. There is no dedicated
Engram obs for testing — the repo docs are the source.

## Repo sources of truth (normative)

- `../../../AGENTS.md` — § Testing: auto-discovery, harness import, the `webfang_path()` golden
  rule, insta snapshot cycle, mandatory sanitization, the six-node diagnostic, how to create an
  integration test.
- `../../../crates/webfang_core/tests/common/cli_harness.rs` — `BehavioralTest` (wiremock +
  TempDir), `scraper_cmd()`, `find_files()`, `read_md_content()`, `webfang_path()`,
  `redact_nondeterministic()`.
- `../../../crates/webfang_core/Cargo.toml` — `[[test]]` entries (only for cases auto-discovery
  does not cover).

## Engram (related context)

| Topic | Query | Obs | Scope |
| --- | --- | --- | --- |
| State persistence / resume (test subjects) | `StateStore ExportState load_or_default version bak` | #216 | project |

Resolve with `engram search "<query>" --project webfang` or `mem_search`.