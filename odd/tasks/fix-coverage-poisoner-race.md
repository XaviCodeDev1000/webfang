# #2341 — la ventana del poisoner de clap no excluye a los parses hermanos

Worktree: `/home/xavi/Projects/Rust/webfang-worktrees/fix-coverage-poisoner-race` · Branch: `fix/coverage-poisoner-race`

## Problema

`cli::args::tests::the_test_parse_surface_ignores_the_ambient_environment` escribe
`WEBFANG_URL=POISON_TRASH` en el entorno del proceso durante una ventana y lo restaura
al final. En la lane de Coverage (`cargo llvm-cov` = libtest con **proceso compartido**)
los demás tests de `cli::args` corren a la vez y parsean con `Args::try_parse_from`, que
lee el `env = "WEBFANG_URL"` declarado por clap: absorbían el veneno y paniqueaban con
`URL inválida: relative URL without a base` en su `.expect("valid args")`.

`clean_env()` era un `Once`: limpiaba una sola vez y nunca más dentro de esa ventana,
así que tampoco cubría a los hermanos. Las lanes nextest (proceso por test) son inmunes,
por eso el rojo solo aparecía en Coverage.

## Causa raíz

Dos huecos independientes, y hacía falta cerrar los dos:

1. **Lectores fuera del guard**: 19 parses del módulo llamaban `Args::try_parse_from`
   directamente, sin el helper hermético. El módulo documentaba que
   `parse_args_hermetic` era «el ÚNICO punto de parseo de la superficie de test»; no era
   cierto.
2. **Ventana sin exclusión**: aunque todos los parses fuesen herméticos, el test del
   poisoner necesitaba exclusividad sobre `set → parse → restore`. `ENV_LOCK` no puede
   darla: `env_set` y el `EnvGuard` del parse lo adquieren por separado y el lock **no es
   reentrante**.

## Fix

- Nuevo `test_support::clap_env_window()`: un lock que abarca la ventana completa y se
  toma **siempre antes** de `ENV_LOCK`, nunca después. Tolerante al envenenamiento
  (`unwrap_or_else(into_inner)`), igual que `ENV_LOCK` (#1788), para que un assert
  fallido dentro de la ventana no arrastre al resto del proceso.
- `parse_args_hermetic` toma la ventana; `parse_args_hermetic_in_window` es la variante
  para quien ya la posee (solo el poisoner, que debe abarcar el set y el restore).
- Los 19 parses directos migran a `parse_args_hermetic`, y `with_clap_env_cleared` deja
  de ser `pub(crate)`: ya no queda una puerta lateral para saltarse el lock.
- Se elimina `clean_env()` con su `Once`: el helper limpia el entorno ambiente en cada
  parse, así que la limpieza única era a la vez insuficiente y engañosa.

## Verificación

- `cargo test -p webfang_core --lib cli::args -- --test-threads=16` en bucle: el
  baseline fallaba 20 de 30 iteraciones (evidencia en la issue); post-fix verde.
- `cargo nextest run -p webfang_core --lib` (equivalente a las lanes nextest).
- `cargo clippy -p webfang_core --all-targets --all-features -- -D warnings` + `cargo fmt --all -- --check`.
