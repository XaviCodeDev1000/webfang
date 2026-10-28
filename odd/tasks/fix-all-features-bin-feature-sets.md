# #2337 — las lanes `--all-features` relinkan el mismo binario con specs distintos

Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-all-features-bin-feature-sets` · Branch: `fix/all-features-bin-feature-sets`

## Problema

En las lanes `--all-features` (`Tests (all features)` y la nightly `Flake detection`)
los dos helpers de test escriben el **mismo** `<target>/debug/webfang` con **feature
sets distintos**:

- `crates/webfang_core/tests/common/cli_harness.rs` deriva por `cfg!()` y compila con
  `adaptive-selectors,mcp,persistence,console,dev-tracing`.
- `crates/webfang_mcp/tests/common/mod.rs` delega en
  `webfang_test_utils::webfang_path()`, que compila con los defaults de `webfang_cli`.

Cada alternancia invalida el link del otro, así que el siguiente test paga un relink
completo dentro de su propio reloj: con `--profile flake-nightly` (retries 0) eso cae
directo en el `slow-timeout` de 180 s de los parity tests.

## Por qué #2335 no bastaba

#2335 añadió un pre-build de **los dos specs** en ambas lanes, pero **sin**
`WEBFANG_TEST_PREBUILT_BIN`: los helpers siguen reconstruyendo por su cuenta, y el
último spec que escribió gana la ruta hasta que el otro vuelve a compilar. Materializar
los dos specs no elimina la alternancia; sólo la hace más barata la primera vez.

## Fix

Se aplica el contrato de #2336 a estas dos lanes: **un binario autoritativo por run**.

- Pre-build del spec que deriva `cli_harness.rs` (los cinco features), una sola vez y
  antes de nextest.
- `WEBFANG_TEST_PREBUILT_BIN: "1"` en el paso de nextest: los dos helpers lo confían y
  ninguno compila dentro de un test.
- Se retira el pre-build `--all-features` de `test-full`: no era el spec que consume
  ningún helper (lo cubre `clippy --all-targets --all-features` en su propia lane).
- Doc de `PREBUILT_BIN_ENV` actualizada: el contrato ya no es sólo «default features».

## Verificación (local, mismo shape que la lane)

Binario: `cargo build -p webfang_cli --bin webfang --features adaptive-selectors,mcp,persistence,console,dev-tracing`
→ `b55619ba…` (sha256).

- `WEBFANG_TEST_PREBUILT_BIN=1 cargo nextest run --all-features -p webfang_mcp -E 'test(cli_vs_mcp)'`
  → 2/2 PASS (`cli_vs_mcp_export_jsonl_record_parity`, `cli_vs_mcp_sitemap_run_record_shape_parity`),
  **hash del binario idéntico antes y después** (cero relink).
- `WEBFANG_TEST_PREBUILT_BIN=1 cargo nextest run --all-features -p webfang_core --test config_default_contract_test`
  → 15/15 PASS, hash idéntico → el helper de `webfang_core` confía el mismo binario.
- YAML parseado (25 jobs).
