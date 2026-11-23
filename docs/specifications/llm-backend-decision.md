# ADR 813: Optimal backend for LLM extraction — OpenAI-compatible vs offline SLM (follow-up #789)

- **Status:** Proposed — option B became unviable with ADR-0004 step 6 (v3.0.0). See the note.
- **Date:** 2026-08-21
- **Issue:** #813 (follow-up #789 / PR #806 `ca602a1`, #702, research `REPORT.md` §2 open question #2, `findings/F2.md` [9][12])
- **Deciders:** Lead Rust Architect (bridge NotebookLM `Senior_Rust_Developer` 6b7f010b…)
- **Scope:** Decision documented with an A/B/C comparison table; **out of scope** implementing the SLM if the benchmark recommends it (own follow-up ODD).

> **Note (2026-10-20).** The **option B** of this document —local SLM over ONNX— is no
> longer implementable. ADR-0004 step 6 retired the Cargo `ai` feature, the `webfang_ai`
> crate and all ONNX inference in v3.0.0 (#2072). The references to `--features ai`, `ort`,
> `hf_hub` and `webfang_ai` below describe the state of 2026-08-21 and are kept as a
> historical record; the A/B/C comparison remains valid only for A and C.

## 1. Context and problem (real code)

`#789` implemented the LLM-First core: trait `LlmPort: Send+Sync { send_completion(LlmRequest)->BoxFuture<Result<LlmResponse,ScraperError>> }` (`domain/llm_port.rs:53`), `OpenAiLlmClient` via `wreq` Chrome145 POST `{base}/chat/completions` with `response_format=json_object` and `temperature=0.0` (`infrastructure/llm/client.rs:63`), `validation.rs` zero-dep (`type/required/properties/enum/items`, paths `$.a.b[0].c`), and `LlmExtractionService` with the pipeline **schema gate → SSRF gate → robots → fetch → cleaner.clean (never HTML to the LLM) → chunk budget `max_tokens*8` → sequential `send_completion` loop → merge dedupe `HashSet<String>` → `validate_record` → envelope** (`application/llm_extraction.rs:44-128`, `CHARS_PER_TOKEN=8`).

Real DI: `Container { llm_port: OnceCell<Arc<dyn LlmPort>> }` (`application/container.rs:117`, `L282 get().cloned()`, `L364 with_llm_port` at-most-once, lazy wiring `#759` so as not to block the MCP handshake), but `LlmExtractionService` stores `Option<Arc<dyn LlmPort>>` for tests (`None` → honest `ScraperError::Config("no LLM provider configured")`). `core/Cargo.toml` feature `ai=[]` empty marker; the ONNX/`ort`/`hf_hub` weight lives only in `webfang_ai` (cache `~/.cache/huggingface/hub`, lazy-download, no `include_bytes!`). `cargo build` without `ai` does not grow; `--features ai` documents Granite 97M ~390MB / 311M ~1.25GB.

The MCP tool `extract_structured` does **not exist** yet in `webfang_mcp/src` in this checkout (`grep` 0 results, `cargo check --all-targets` green) — the benchmark is validated at the `webfang_core` level.

Offline-first vs OpenAI-compatible tension: local inference is massively CPU-bound (if it runs on the Tokio executor → *thread starvation* → it needs `spawn_blocking` via `CpuBridge`), while `OpenAiLlmClient` is pure `wreq` I/O. The hierarchy of truth is **compiler > real code > NotebookLM**; the mentor contributes theory, the `borrow checker` and `cargo check` decide.

Research reports: quantized SmolLM2-360M ~90MB extra, 70-85% valid JSON in single-entity schemas without post-processing, vs GPT-4 11.97% invalid in complex schemas → PARSE 82.3%→98.7% with refinement (`F2.md` [9][12]). Hybrid C `generate_schema() → CSS/XPath` reusable at zero recurring cost (`F2.md` [4]) would cover >80% of stable cases.

## 2. Bounded benchmark (no model implementation)

**Goal:** measure the 3 options with the **same 3 fixtures** and decide default/feature flags without bloating the base binary (`ai` or `llm-local` feature-gated, lazy `hf_hub`).

### 2.1 Fixtures (schemas that pass `validate_schema`)

```json
// A) Single flat entity (product) — valid_json_rate 70-85% expected on SLM
{
  "type": "object",
  "properties": {
    "name": { "type": "string" },
    "price": { "type": "number" },
    "currency": { "type": "string", "enum": ["USD","EUR","ARS"] }
  },
  "required": ["name","price","currency"]
}

// B) Homogeneous list — array of objects
{
  "type": "array",
  "items": {
    "type": "object",
    "properties": {
      "id": { "type": "integer" },
      "title": { "type": "string" }
    },
    "required": ["id","title"]
  }
}

// C) Complex nesting 3 levels deep (company → departments → employees)
// Note: the zero-dep validator recurses over `object.properties` but in `array.items`
// it only validates a simple `type` string (see `validation.rs`); depth 3 is validated
// superficially and the rest is covered by PARSE-like refinement.
{
  "type": "object",
  "properties": {
    "company_name": { "type": "string" },
    "departments": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "department_name": { "type": "string" },
          "employees": {
            "type": "array",
            "items": {
              "type": "object",
              "properties": {
                "employee_name": { "type": "string" },
                "role": { "type": "string" }
              },
              "required": ["employee_name","role"]
            }
          }
        },
        "required": ["department_name","employees"]
      }
    }
  },
  "required": ["company_name","departments"]
}
```

### 2.2 Harness (deterministic, no real network and no API keys)

- **Location:** `tests/behavioral/llm_benchmark.rs` (or `crates/webfang_core/benches/llm_bench.rs` if `cargo bloat` is measured); inherits `BehavioralTest` + `TempDir` + `wiremock`.
- **Option A (external API):** `wiremock::MockServer` with `POST /chat/completions` → pre-recorded responses + corrupt corpus (11.97% invalid JSON, `finish_reason=length`). `WEBFANG_DISABLE_SSRF=1` for loopback. Measures real `input_tokens:u32`/`output_tokens:u32` from `Usage`.
- **Option B (local SLM):** deterministic `StubLlm: LlmPort` with `spawn_blocking` (simulates `ort` CPU) + `std::thread::sleep(120ms)` and `content.len()/8` as `output_tokens`; 100 iterations of corpus validated against `validate_record`.
- **Option C (hybrid):** stub `generate_schema()` → synthetic reusable CSS/XPath; measures `<5ms` native parsing + 0 LLM cost.
- **Metrics per fixture:** `valid_json_rate = valid / iterations *100`, `p50 latency ms` (sort latencies), `chunks`, `input/output_tokens`, `binary +MB` (`cargo build --features ai` vs base), `cost per 1k requests`.
- **Observability:** `tracing::info!(backend=%backend, model=%model, chunks, valid_rate, p50_ms, "benchmark tick")` + `#[instrument(fields(backend, model))]` on `extract`; internal `correlation_id` `skip` in snapshots via `redact_nondeterministic()` (filters `p50_ms`, `tokens`).
- **Rust invariants:** `!Send` `scraper::Html` never crosses `.await` — it is confined to a synchronous block or `spawn_blocking`; `LlmPort: Send+Sync` guarantees `BoxFuture +Send`.

## 3. Comparison table and recommendation

| Dimension | A: External API (OpenAI/Ollama, already exists) | B: Local SLM SmolLM2-360M quant ~90MB | C: Hybrid `generate_schema() → CSS/XPath` |
|---|---:|---:|---:|
| **Cost per 1k requests** | variable pay-per-use (≈ $0.001-0.05/extract) | USD 0 (local compute) | USD 0 |
| **p50 latency** | 800-1500 ms (network) | 1200-3000 ms (CPU `spawn_blocking`) | **<5 ms** (native parsing) |
| **valid_json_rate** | ~88% (GPT-4 nested, 11.97% invalid without PARSE → 98.7% with refinement) | **70-85% single entity only** (drops on B/C) | **100%** (guaranteed by the parser) |
| **Binary +MB** | +0 MB | +90MB model + `ort` (only with `--features ai`) | +0 MB |
| **Feature flag** | always available by default | `ai` (lazy `hf_hub`) | `adaptive-selectors` (already exists, cascade trace) |
| **Offline** | no (requires `LLM_API_URL`+`LLM_API_KEY`) | **yes** (offline-first) | **yes** |

**Default recommendation (respecting that `cargo build` without `ai` does not grow):**

> **Default = hybrid C as the first layer** (`generate_schema()` one-time → reusable CSS/XPath, latency <5ms, 100% valid, 0 cost). **Dynamic fallback → A** via `OpenAiLlmClient` (OpenAI/Ollama) when selectors drift or the schema is not covered, with a strict `ssrf_gate` and `response_format=json_object`. **B (SLM) strictly opt-in** under `--features ai` with lazy `hf_hub` to `~/.cache/huggingface/hub` (no `include_bytes!`), only if the user needs full offline mode and accepts 70-85% without refinement.

**Feature flags:** **Reuse the `ai=[]` marker** (do not create a separate `llm-local`). Justification: `ai` already isolates `ort`/`hf_hub`/`tokenizers` in `webfang_ai`; adding `llm-local` would duplicate the matrix and CI with no benefit (90MB is already the documented cost of `ai`). The `cargo build --features ai` benchmark must document `+MB` and the base `cargo build` stays light (DX). If a follow-up needs Granite and SmolLM2 separately, introduce `ai-llm` as a sub-feature, not now.

**Operational guide (honest doc):**
- Offline without LLM → `extract` returns `Config("no LLM provider configured")` (honest error).
- With external API → configure `LLM_API_URL` + `LLM_API_KEY` (or `OLLAMA_URL=http://localhost:11434/v1` with `LLM_API_KEY=dummy`).
- With offline SLM → `cargo build --features ai` + `WEBFANG_AI_MODEL_ID=HuggingFaceTB/SmolLM2-360M` (or the inherited `AI_MODEL_ID`) → lazy `hf_hub` download.

## 4. Mitigations and risks

- **SSRF:** `ssrf_gate(&Url)` allow-list `http/https` + `is_forbidden_literal_host` (loopback/private/link-local/CGNAT/ULA); tests `WEBFANG_DISABLE_SSRF=1`.
- **Token budget without overlap:** `char_budget = max_tokens*8` validated before the LLM (over-budget → `Validation`); no `overlap_rate` today → risk of a record being cut at a chunk boundary (mitigate with PARSE-like refinement: re-inject the `SchemaError` path `$.a.b[0].c` to the LLM in a bounded retry).
- **Dedupe:** `HashSet<String>` over `record.to_string()` → sensitive to key order/spacing; document as a limitation and recommend canonical normalization in a follow-up.
- **Zero-dep validation:** no `$ref`/`allOf`/`format`; deep fixture C requires post-validation with Serde or a refinement loop.
- **`!Send` Html:** never hold `scraper::Html` (internal Rc) across `.await`; isolate it in `spawn_blocking` or a synchronous block before `send_completion`.

## 5. Tests (no test against a real network or keys in the repo)

- `wiremock` for A (200/429/503/length), `StubLlm` with `spawn_blocking` for B, `generate_schema` stub for C.
- 3 fixtures × 100 iterations → `valid_json_rate` + `p50` + `insta::assert_snapshot!(redact_nondeterministic(...))`.
- Honest error: `LlmExtractionService::new(..., None)` → `extract` == `Config`; `ssrf_gate("http://127.0.0.1")` blocked vs `WEBFANG_DISABLE_SSRF=1` allows wiremock.

## 6. Out of scope for this issue

Implement the bundled SLM if the benchmark recommends it (goes in a follow-up with its own ODD, including `cargo bloat` and `ort` session sharing `Arc<Mutex<Session>>` `#648`).

## 7. Audit log (3 rounds with NotebookLM `Senior_Rust_Developer`)

| Round | NotebookLM proposal | Local code reality | Adjustment applied |
|---|---|---|---|
| R1 | IO vs CPU tension, `Container Option`, harness mock `usize` tokens + overlap | `container.rs:117 OnceCell` not Option; `LlmResponse u32`; `CHARS_PER_TOKEN=8` without overlap; `cargo check` green | Correct types to `u32`, `OnceCell::new()`/`get().cloned()`, document no overlap |
| R2 | Harness `spawn_blocking` + flat/array/nested fixtures + reuse `ai` vs `llm-local` | `LlmExtractionService Option` (not OnceCell), `ChatMessage` Serialize only, `validation.rs` without deep array recursion, `adaptive-selectors` is not LLM | Distinguish Service Option vs Container OnceCell; corrected schemas `Clone Serialize`; note validator limitation; `ai=[]` marker reused |
| R3 | `docs/llm-backend-decision.md` full ADR + `OnceCell` vs `Option` invariants + `!Send` Html + PARSE refinement | `extract_structured` MCP does not exist yet (grep 0); empty `core ai=[]`, weight only in `webfang_ai` | ADR with table, harness in `tests/behavioral`, observability `redact_nondeterministic`, default decision C→A fallback, B opt-in `ai` lazy `hf_hub` |

## 8. Verified acceptance criteria

- `cargo build` without `ai` does not grow (empty core marker, ONNX only in `webfang_ai`).
- `cargo build --features ai` documents the Granite/SmolLM cost ~90MB-1.25GB lazy.
- `extract` without config → honest error `Config("no LLM provider configured")`.
- Doc present in `docs/llm-backend-decision.md` with table, fixtures, valid_rate, p50, +MB, recommendation and flags.

---
*Generated via the `OMAR-DEV` bridge notebook `Senior_Rust_Developer` (6b7f010b-7122-4757-a96e-b4fade1043a0), conversations `ea00fcdf-2614-4e31-ab0c-bf03b80e621a` (R1-R3), notes `88e9c42d/65a15f7e/2d0aceba`, `gh issue 813`.*