# dead-seams-2351

Objective: remove the four abstraction seams the October 2026 Rust audit found with zero production call sites (issue #2351), and give the one seam that still guards a real capability — `TextChunker` — a production implementation instead of deleting the capability with it.
Branch: `chore/dead-seams-2351` · Worktree: `~/Projects/Rust/webfang-worktrees/chore-dead-seams-2351` · Delivery: PR (owner instruction, supersedes L1) · Runner: `cargo nextest`.

## Specs

- **S1** — `ScraperPort` exists once or not at all. The module payload survives: `ExtractResult`, `author_extractor`, `dom_pruner`, `fallback` and `readability` are still used in production.
- **S2** — `PersistencePort`: deleted, with its `#[cfg(test)]` mock. Real persistence runs through `CrawlResultRepository`/`RecordStorePort`.
- **S3** — `TextChunker`: kept as a port and given a production adapter (`MarkdownChunker`), wired into `VaultAiPorts` by the CLI, with a unit test that exercises it. Deleting the trait would have meant deleting the chunking capability of `VaultSearchService` and `LlmExtractionService` too, which is a different (and larger) mission.
- **S4** — `OutputStage`: deleted. The engine consumed it, production built `Vec::new()`, and the only implementations were test mocks that tested the extension point against itself. `content_sink` already writes every item, so the seam was redundant.
- **S5** — `cargo check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo fmt --all -- --check` and `RUSTDOCFLAGS=-D warnings cargo doc --workspace --all-features --no-deps` green.

## Tasks

- **T1** — `domain::ports`: drop `ScraperPort` and `PersistencePort` plus their mocks. **done**.
- **T2** — `domain::scraper_port` renamed to `domain::html_extraction`: the module holds extraction primitives, not a port, and the old name kept implying a trait that no longer exists. Trait + `FakeScraper` tests deleted, payload untouched. **done**.
- **T3** — `application::pipeline::stages::output` deleted with its plumbing: `CrawlTaskCtx.output_stages`, `CrawlPorts.output_stages`, `Engine.output_stages`, `write_to_output_stages` and the two tests that only exercised the extension point. **done**.
- **T4** — `infrastructure::text_chunker::MarkdownChunker`: paragraph packing under a character budget, hard split for oversized blocks, budget clamped at 64 characters. Wired in `build_ai_cleaner` from `--max-chars`. **done** (5 unit tests).
- **T5** — `tools/intra-crate-lint` golden set refreshed in the same commit that changes the tree, as its module doc requires. **done**.

## Log

- **L1** — Owner instruction, verbatim: «vamos a implementar las issues abiertas 2351/2352/2353/ cada una en su propio worktree y no abrimos pr's».
- **E1** — Call-graph check before deleting: `ScraperPort` had two definitions and 0 call sites; `PersistencePort` had 0 production impls; `OutputStage` had 4 production construction sites, all `Vec::new()`; `TextChunker` had production *consumers* but no production *impl*.
- **E2** — `VaultAiPorts.text_chunker` was never populated by any binary: the MCP tool `search_obsidian` bailed on `chunker de texto` unconditionally, even when an embedding provider had resolved. Wiring the chunker removes that gate; `note_repository` is still unset in production and remains the next blocker for the tool (out of scope here).
- **E3** — `webfang_core` lib tests: 2982 passed. `webfang_cli` + `webfang_mcp` lib tests: 368 passed.
- **E4** — `intra-crate-lint` tests green after refreshing `real_tree_sites.txt` (the golden set shifted by the renames and the deletions).
- **E5** — `benches/readability.rs` imported `domain::scraper_port::dom_pruner`; updated to the new path. The bench is not covered by `cargo check -p`, only by `--all-targets`.
- **L2** — Owner instruction, 2026-10-26, verbatim: «primero abrimos las pr's de los otros worktree si PR abierta». Delivery updated from local branch to PR.
