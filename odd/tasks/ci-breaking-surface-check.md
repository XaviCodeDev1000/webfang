# ci-breaking-surface-check

Check requerido de CI para el invariante de ruptura (PR head + merge_group).
Issue: #2328 (status:approved). Branch: `ci/breaking-surface-check`.

## Specs

S# "Un contexto requerido (p. ej. `breaking-surface`) que en `pull_request` lea título + commits + `headRefOid` y aplique la misma matriz que `breaking_guard`."
S# "El mismo invariante corre en `merge_group`: obtener el número de PR desde `merge_group.head_ref` (`gh-readonly-queue/<base>/pr-<N>-<sha>`) y fallar cerrado si no se puede resolver."
S# "Lee el método de merge de la cola (GraphQL) en vez de asumirlo; no depende de flags del script."
S# "Metadata incompleta, inválida o inesperada -> check rojo (nunca verde por defecto)."
S# "Pagina los commits: `gh pr view --json commits` corta en 100, así que un marcador en el commit 101 no puede quedar invisible."
S# "Tests offline para la matriz completa y para ambos eventos, incluyendo metadata incompleta y el cap de 100."
S# "El guard local de `scripts/merge-when-green.sh` se mantiene como defensa en profundidad; este issue no lo elimina."
S# "Documentado en AGENTS.md y en la ayuda del script. Fuera de alcance: cambiar el método de merge de la cola."

## Tasks

- [ ] Extraer la matriz de breaking_guard a `scripts/breaking_surface_lib.sh` (regexes, subject_kind, conteo, veredicto puro) y refactorizar merge-when-green.sh para consumirla (defensa en profundidad intacta; el harness existente es la red de seguridad).
- [ ] Nuevo `scripts/check_breaking_surface.sh`: resuelve PR, lee método de merge de la cola vía GraphQL, pagina commits vía REST `--paginate` (fix del cap de 100), valida shape fail-closed, aplica la matriz de la lib; modo `--fixture` para tests offline.
- [ ] Nuevo job `breaking-surface` en pr-validation.yml (pull_request + merge_group), resolución de PR fail-closed (mismo parse sintético que `validate`).
- [ ] Harness `scripts/test_breaking_surface.sh` (offline, fake gh + fixtures): matriz completa, ambos eventos, metadata incompleta, cap de 100, método de cola SQUASH/MERGE/REBASE/ausente; marcador `# NEGATION:`; pasa intacto y rojo con el guard mutado (meta-guard #2056).
- [ ] Cablear el harness en ci.yml (repo-guards) y actualizar `test_pr_validation_merge_group_ref.sh` para el nuevo paso de resolución.
- [ ] Docs: AGENTS.md (sección breaking change) + ayuda de merge-when-green.sh (defensa en profundidad, check requerido).
- [ ] Verificación: bash harnesses + shellcheck + actionlint (sin cargo: el cambio no toca Rust).

## Log

L1 = #2328: "Mover el invariante de ruptura de `scripts/merge-when-green.sh` a un check requerido de CI que lo valide en `pull_request` (head del PR) y en `merge_group` (entrada de la cola), para que no dependa de que el merge lo haga el watcher local." — CI cierra el TOCTOU (ventana entre el segundo chequeo y el enqueue/merge) y el hueco de merges que no usan el script (UI, gh directo).
