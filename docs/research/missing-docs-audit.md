# missing_docs audit

## Summary

> **Historical baseline of the raw audit** (with double counting per module):
> - webfang_core: 206 warnings
> - webfang_ai: 213 warnings
> - Total: 419 warnings
>
> This number includes duplicates across modules. The real deduplicated count is 199.

- deduplicated warnings: **199** → **0** (post #257/#258)

## By module

| Module | Warnings |
|---|---:|
| cli | 74 |
| infrastructure/crawler | 44 |
| cli/args | 40 |
| application/crawler | 36 |
| adapters | 36 |
| domain | 34 |
| domain/error | 30 |
| application | 30 |
| infrastructure/downloader | 22 |
| domain/entities | 16 |
| infrastructure | 12 |
| application/batch | 12 |
| infrastructure_ai | 7 |
| application/pipeline/stages | 6 |
| adapters/downloader | 6 |
| adapters/detector | 6 |
| (crate root) | 6 |
| domain/site | 2 |

## Top files

| File | Warnings |
|---|---:|
| adapters/url_path.rs | 36 |
| domain/error/crawl_error.rs | 30 |
| cli/error.rs | 30 |
| cli/args/mod.rs | 30 |
| application/crawler/crawl_task_ctx.rs | 30 |
| cli/export_flow.rs | 26 |
| infrastructure/crawler/sitemap_parser.rs | 24 |
| domain/mod.rs | 24 |
| application/progress_types.rs | 24 |
| domain/entities/content.rs | 16 |
| infrastructure/downloader/cookie_bridge.rs | 12 |
| infrastructure/autotuning.rs | 10 |
| cli/mod.rs | 8 |
| application/batch/manager.rs | 8 |
| infrastructure/crawler/compression_handler.rs | 6 |
| config.rs | 6 |
| cli/commands.rs | 6 |
| application/pipeline/stages/output.rs | 6 |
| application/crawler/collector.rs | 6 |
| adapters/downloader/mod.rs | 6 |

## Implementation recommendation

Estimated effort: L

Justification based on the total number of warnings:
- Total 100-500 → M (2-3 PRs per module)

Suggested phase plan:
1. Domain (traits, entities, value objects, errors, invariants)
2. Application (services, use cases, orchestration)
3. Ports/config/errors (adapter interfaces, repositories, public config types)
4. webfang_ai (providers, prompts, schemas, integration errors)
5. CI + final deny

## Criteria to enable `deny`

- Total < 100 warnings → enable deny in the next documentation PR.
- Total 100-500 → enable deny in phases, deny only in already-documented modules.
- Total > 500 → do not enable deny until an approved phased documentation plan exists.

## Technical notes

- `missing_docs` default is `allow`. It was changed to `warn` for the audit.
- `deny` was not enabled in this phase.
- No lint configuration item in Cargo.toml was modified.

## Classification correction

The original classification was **M**, but the total of 419 warnings exceeds the 200-warning
threshold defined in the brief.
Therefore, the correct effort is **L** and it corresponds to implementing the documentation
in phases, not in a single PR.

## Addendum: Raw → deduplicated reconciliation (2026-07-24)

The original audit reports **419 raw warnings**:

- `webfang_core`: 206
- `webfang_ai`: 213
- Raw total: 419

However, `webfang_ai` depends on `webfang_core`. When `#![warn(missing_docs)]` is enabled
in `webfang_ai`, the 206 warnings from `webfang_core` propagate. Only **7 warnings** are
really from `webfang_ai` (in `infrastructure_ai/`).

The `audit-missing-docs.sh` script was fixed to deduplicate by `file:line`, eliminating the
double counting. Effective state:

| Metric | Raw | Deduplicated |
|:--------|------:|:-----------:|
| webfang_core | 206 | 192 |
| webfang_ai (own) | 213 | 7 |
| **Total** | **419** | **199** |

### Breakdown per PR (Phase 0)

| PR | Modules | ΣWarnings | ΣE (pub(crate)) | ΣR (docs/allow) |
|:--:|:--------|:---------:|:----------------:|:----------------:|
| 0A | (no warnings — tooling + lint swap) | 0 | 0 | 0 |
| 0B | infrastructure/*, adapters/* | 62 | ~50 | ~12 |
| 0C | cli/*, cli/args/* | 50 | ~35 | ~15 |
| 0D | domain/*, application/*, config.rs | 80 | ~42 | ~38 |
| F5 | webfang_ai/infrastructure_ai | 7 | 0 | 7 |
| **Total** | | **199** | **~127** | **~72** |

**Post-Phase-0 (real):** 90 eliminated, 109 remaining for Phases 1-5.

### Impact on effort

The effort is revised from **L** to **M**, given that:
- Real unique warnings: 199 (not 419)
- Eliminated via `pub(crate)` (Phase 0): 90
- Remaining for documentation (Phases 1-5): **109**

## Reproduction

To reproduce the deduplicated count:

```bash
./scripts/audit-missing-docs.sh
```

The script automatically deduplicates by `file:line` using `sort + awk`.

## Final status (post #257 / #258)

- missing_docs warnings: **0** (199 dedup → 0)
- Doctests: **86 passed**, 7 ignored (justified per item), 0 failed
- deny(missing_docs): **active** in webfang_core and webfang_ai
- `[package.metadata.docs.rs]`: configured in both crates
- CI: 3 docs quality jobs (rustdoc, doctests, workspace check+test)
- Miri: 3 FFI tests ignored (BoringSSL), the rest PASS
- Snapshot tests: `#[cfg(feature = "ai")]` gating + `webfang_path()` with `--all-features`