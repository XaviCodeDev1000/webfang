# Release Feature Matrix

Every release publishes **three binaries** — the `webfang` CLI and the two MCP
servers (`webfang-mcp`, `webfang-mcp-stdio`) — and every binary is built from
the **same feature set, always**. This page answers *"what can the binary I
just downloaded do?"* capability by capability, without you having to read the
release workflow.

The capabilities below are **compile-time Cargo features**, not runtime
configuration: if a release binary was built without a feature, the capability
is not in the file, and no flag combination brings it back. The bottom of this
page carries a machine-readable manifest of the feature set, and CI
drift-checks it against the `[features]` table of
`crates/webfang_cli/Cargo.toml`
(`scripts/check_release_feature_matrix.sh`, a required step of the
`repo-guards` job) — so this page cannot silently rot when a feature is added
or removed.

> **What is NOT covered here:** which *platforms* get artifacts (see
> [Installation](installation.md#1-pick-your-asset)) and how to verify a
> download. This page is only about compiled-in capability.

---

## The three artifacts

| Binary | Crate | What it is |
| :--- | :--- | :--- |
| `webfang` | `webfang_cli` | the scraper CLI |
| `webfang-mcp` | `webfang_mcp` | MCP server, Streamable HTTP on `127.0.0.1:8080/mcp` |
| `webfang-mcp-stdio` | `webfang_mcp` | MCP server over stdin/stdout (JSON-RPC), for desktop clients |

The exact release build is **one invocation**
(`.github/workflows/release.yml`, step "Build release binary"):

```bash
cargo build --release --locked \
  --features "webfang_cli/mcp" \
  --target <target-triple> \
  -p webfang_cli -p webfang_mcp --bins
```

No capability feature is enabled: `webfang_cli`'s `default = []`, and
`chromium`, `persistence`, `console` and `adaptive-selectors` are capability
features that no release artifact carries. `webfang_cli/mcp` is an
**explicit-empty marker** (`mcp = []`): the key must exist because the CLI
test harness and several scripts build with `--features mcp`, but it selects
nothing — it puts **zero MCP code** into the `webfang` binary. The MCP
surface lives in the `webfang_mcp` crate, which is built as its own two
binaries and shipped in its own archives.

---

## Capability matrix

| Capability | `webfang` CLI (release) | MCP servers (release) | Build from source |
| :--- | :--- | :--- | :--- |
| MCP (36 tools) | **Not embedded.** The CLI exposes no MCP flag. The servers are the two separate binaries — see [MCP servers](installation.md#mcp-servers-from-the-release-artifact). | **Included** — this is what those binaries are. | `cargo build -p webfang_mcp --bins` |
| `--js-strategy full` (Chromium rendering) | **Rejected at preflight** with an honest error — the Chromium downloader is not compiled in. The default `static` strategy and the 3-layer `hybrid` strategy are unaffected. | The `get_accessibility_snapshot` tool is registered but answers with a feature-gated error instead of driving a browser. | `--features chromium` (needs a Chromium/Playwright runtime at run time) |
| SQLite persistence | **Absent.** The `crawl.db` checkpoint store does not exist; file-based `--resume` state is unaffected. | Absent. | `--features persistence` (enables the SQLite store; `--elastic` / `--db-path` ingestion paths use it) |
| Adaptive selector learning | **Absent** — the flags do not exist in this build. | Absent. | `--features adaptive-selectors` |
| tokio-console | **Absent.** Tracing via `--trace-file` is unaffected. | Absent. | `--features console` |
| `dev-tracing` | **Never in a release** — a development aid, not a capability. | Never. | `--features dev-tracing` |

Historical note: the former `ai` feature (local ONNX semantic cleaning) was
**removed** in ADR-0004 paso 6, not feature-gated — semantic cleaning is
runtime-configured through `--embedding-provider` in every build, and
`--offline` rejects every provider. See the
[migration guide](ai-onnx-migration.md).

---

## The machine-readable contract

This manifest is the guard's input, not reader-facing prose: it is parsed by
`scripts/check_release_feature_matrix.sh` and must mirror the `[features]`
table of `crates/webfang_cli/Cargo.toml` **exactly** — every feature key
present once, `default` absent, and each `kind` matching how the feature is
declared (`capability` = forwards a `webfang_core` feature that gates code or
dependencies; `marker` = an explicit-empty feature that selects nothing).
Adding or renaming a feature in the manifest without touching `Cargo.toml`
— or the other way around — fails the `repo-guards` job.

<!-- feature-matrix:begin -->
```feature-matrix
chromium           | capability | --js-strategy full rejected at preflight (#685)
mcp                | marker     | explicit-empty marker (#1815 T4); MCP ships as the webfang_mcp binaries
persistence        | capability | SQLite store absent from release artifacts
console            | capability | no tokio-console in release artifacts
adaptive-selectors | capability | flags absent from release artifacts
dev-tracing        | capability | development tracing, never in a release
```
<!-- feature-matrix:end -->
