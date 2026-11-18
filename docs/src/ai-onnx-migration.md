# Migration from local ONNX

Local ONNX **no longer exists** in the product (ADR-0004, step 6). This page says **what
changed, who it affected and what to do now**, in step order. The decision and its sequence
are in
[ADR 0004](https://github.com/XaviCodeDev1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md).

## What changed and when

| Moment | What happened |
| :--- | :--- |
| Notice (2 minors before the cut) | It was **announced**. Nothing was deleted: the shims kept working and `--clean-ai` kept using local ONNX |
| Migration window | `--embedding-provider` resolved the vault search slot while ONNX was still alive |
| Cut (step 6 of ADR-0004) | Local ONNX, the `webfang_ai` crate and the `ai` feature **were removed**. This is the current state |
| Shim removal | The migration shims **were removed**: they no longer parse. A retired flag answers with the standard clap error (`unexpected argument`, exit 64) |

`cargo build --features ai` now fails at feature resolution: the feature is gone and leaves
no shim behind. A shim behind a feature that gets deleted does not exist, in any language.

## Who is affected

| Your setup | What happens to you now |
| :--- | :--- |
| `--clean-ai` with the local model (Granite 97M / 311M) | **This affects you.** The cleaner resolves its endpoint through `--embedding-provider`. You lost full offline mode |
| `--clean-ai` + `--embedding-provider` already configured | Works today: the flag moves the vault search slot **and** feeds the cleaner through the ungated `EmbeddingPort` |
| `--ai-model`, `AI_MODEL_ID`, `WEBFANG_AI_MODEL_ID`, `WEBFANG_AI_ENGINE` | **Retired.** They are no longer flags: `--ai-model` answers with the standard clap error (`unexpected argument`, exit 64) and the env vars are ignored silently. Use `--embedding-provider` / `WEBFANG_EMBEDDING_PROVIDER` |
| `--max-tokens`, `WEBFANG_MAX_TOKENS` | **Retired.** `--max-tokens` no longer parses (standard clap error, exit 64) and `WEBFANG_MAX_TOKENS` is ignored. Use `--max-chars` / `WEBFANG_MAX_CHARS` |
| Vaults with Granite vectors | You must **re-index**: the new endpoint does not produce comparable vectors. You pay the cost (and the BYO key) |
| You do not use the AI surface | Nothing. The notices do not appear in runs that never touch it |

## How to migrate

1. **Configure the remote endpoint** — it detects and validates, and writes the config:

   ```bash
   webfang connect
   ```

   Check that it came out right, with the detail of every value:

   ```bash
   webfang doctor
   webfang show
   ```

2. **Point `--clean-ai` at the remote endpoint — available since the cut.**

   The cleaner resolves its `EmbeddingPort` through `--embedding-provider`
   (or its env `WEBFANG_EMBEDDING_PROVIDER`; the flag wins over the env):

   ```bash
   webfang --url https://example.com --clean-ai --embedding-provider ollama
   ```

   Under `--offline` there is no embeddings provider that works: the guard rejects any
   endpoint — a public remote one or one on `127.0.0.1` — and exits with 78. The "allowed
   loopback" that the step 5 notice mentioned referred to the local ONNX model, which this
   step removed; the notice no longer says it and now names the real rule (it rejects any
   embeddings provider, remote or loopback). Without that path, `--offline` is only good for
   runs without semantic cleaning. A LAN endpoint still needs the CIDR allowlist
   (`allow_cidrs`), which does **not** include `100.64.0.0/10` unless you opt in explicitly.

3. **Re-index the vaults** that have Granite vectors. There is no way to detect the mix: the
   fail-closed by dimension cannot fire on the vaults that never had a `model_id`, which are
   exactly the affected ones. You will see a "legacy" notice; re-indexing is what closes it.

4. **Rename `--max-tokens` → `--max-chars`.** The shim was removed: `--max-tokens` no longer
   parses and `WEBFANG_MAX_TOKENS` is ignored silently.

## What still works

`--clean-ai` and `--offline` **started as always-on flags** at the cut: they were ungated
and remained in the default build, with no feature. `--clean-ai` changed semantics
(ONNX → remote endpoint); it was not a shim, it is the same flag doing the work with a
different backend.

## The 0.3 threshold

The default `0.3` was calibrated for Granite. With the only calibrated model retired, a
remote endpoint retains differently with the same number, so the run warns once while the
threshold is the default. The notice is not decoration: it is the signal that the number no
longer means what it said.

Pick a threshold for your model:

```bash
webfang --url https://example.com --clean-ai --embedding-provider ollama --threshold 0.45
```

The effective value of each run appears as `threshold_used` in the summary.

## If you still need local ONNX

The deletion is reversible **from a git recovery tag created before the deletion**:
tag `onnx-tree/pre-step5` (commit `38ab6a31`). Recovering that tree is the only official path
to the original ONNX; from there, a revival requires a new ADR — it is not a rollback and is
not implemented on the current line.

References: [CLI reference](cli-reference.md) ·
[ADR 0004](https://github.com/XaviCodeDev1000/webfang/blob/main/docs/adr/0004-onnx-local-removal.md)