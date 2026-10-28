# ci-breaking-surface-required

Issue: #2412 (status:approved). Branch: `ci/breaking-surface-required`.
Base: main @ 84c5230c. Worktree: `webfang-worktrees/ci-breaking-surface-required`.

## Specs

S# "Registrar el contexto requerido `breaking-surface` (el job de `pr-validation.yml` de #2328, mergeado vía #2405) en la branch protection / ruleset de las ramas base (`main`, y las líneas de release/support activas si aplica)."

S# "`breaking-surface` figura en los required status checks del ruleset de `main` (registrar también en `release/X.Y` / `support/X.Y` solo si esas líneas reciben PRs con breaking changes)."

S# "Un PR con marcador de ruptura en superficie perdida queda BLOCKED por el check (verificar con un caso de prueba o revisando el run de #2405)."

S# "El guard local `scripts/merge-when-green.sh` sigue en pie (defensa en profundidad, #2328 S7); nada que eliminar."

## Tasks

- [x] Análisis: el check NUNCA pasó — run 40594357225 (PR #2420): exit 126, `scripts/check_breaking_surface.sh: Permission denied`. Modo del archivo 100644 (hermanos invocados igual: 100755).
- [x] Registrarlo requerido con el script roto bloquearía todos los PRs → prerequisite: chmod +x (este PR).
- [x] chmod +x + harness `test_breaking_surface.sh` 49/49.
- [ ] PR mergeada por la cola.
- [ ] Registrar `breaking-surface` como required context en el ruleset `main-protection-migrated` (25720640). Solo `main`: `support.json` no tiene ninguna rama release/support activa (`branch: null` en todas las líneas).
- [ ] PRs abiertos sin reportar el check (#2419 #2385 #2362 #2356 #2350): se auto-reparan en el próximo push (`synchronize`) o al entrar a la cola (`merge_group`); no se tocan sus branches.
- [ ] Verificación final (GET ruleset) + cierre de #2412 con evidencia.

## Log

L1 = "Issue #2412 analizar y revisar luego implementar, pero ten cuidado en saturar los recursos del sistema cuando ejecutas comandos pesados, utiliza context7 y jina en caso de necesitaa investigar alguna duda"

L2 = Análisis: el issue asumía "todos los PRs activos ya lo reportan" — falso hoy (4-5 de 6 abiertos sin el check en su head) y el único que lo reporta (#2420) está en FAILURE por exit 126 (bit de ejecución). `merge_group` sí dispara el check en cada entrada de cola, así que el registro requerido no cuelga PRs de forma permanente.

L3 = Fix: chmod 100644→100755 en `scripts/check_breaking_surface.sh`. Diff de modo puro: sin build local (nada que compilar); verificación proporcional = harness de semántica 49/49.

L4 = Colisión resuelta: una segunda sesión de agente escribió este mismo registro de misión (23:34) replicando el análisis de esta sesión; sin commits propios. El usuario no respondió la consulta de propiedad; esta sesión continúa el delivery (mayor avance, guards de git hacen la carrera visible). Registro adoptado como misión compartida.

L5 = Pivot: #2423 (misión fix-llm-artifact-coverage) aterrizó el mismo chmod y quedó MERGED → PR #2422 cerrada como superseded. Branch sincronizada con main (merge 4d333206).

L6 = Diagnóstico del check en runner (post-chmod): 4/4 corridas mueren instantáneas (~560ms: start 23:14:22.10 → error .67) en la lectura GraphQL de la cola, con REST funcionando en el mismo runner y el mismo query respondiendo SQUASH al instante con token admin local. GITHUB_TOKEN es una integración sin permiso de administración → hipótesis "Resource not accessible by integration" sobre Repository.mergeQueue.configuration. El 2>/dev/null del script tapaba la causa.

L7 = Fix en branch: `administration: read` en las permissions del job + fail-loud (stderr de gh se surfacea en el die). Harness 49/49 y meta-guard FAIL=0 tras los edits. Experimento en PR nueva; si `administration: read` no es clave válida para GITHUB_TOKEN en repo público, el workflow muere ruidosamente solo en esta PR y se pivota (PAT secret).

L8 = Experimento de permisos fallido y probado: `administration` NO es un scope de GITHUB_TOKEN (actionlint; el commit d2baadbd invalidó el workflow y la corrida pull_request ni existió). Revertido en 068cccf5. El fail-loud (stderr de gh en el die) reveló el error real en la corrida de 00:00:14Z: FORBIDDEN "Resource not accessible by integration" sobre repository.mergeQueue.configuration — GITHUB_TOKEN jamás podrá leer la config viva.

L9 = Diseño final (patrón support.json): espejo trackeado .github/merge-queue.json (main->SQUASH), reader compartido breaking_queue_method_from_mirror en breaking_surface_lib.sh (check + guard no pueden drift), check lee el espejo fail-closed, guard cross-chequea espejo vs LIVE en el merge boundary y se niega en divergencia. Lib sourced al tope del guard (el cross-check corre en queue mode antes del breaking-guard). Harnesses: breaking 55/55, queue FAIL=0 con tests de divergencia, breaking-guard 56/56. e2e local: OK estrategía=squash.

L10 = Colisión 2: la otra sesión commiteó y pusheó el trabajo compartido como c1f0e2b7 ("fix(ci): mirror the merge-queue method...") — contenido íntegro (marcas clave verificadas, harnesses verdes contra ese árbol). Adoptado; PR #2426 retitulada al diseño final. El veredicto de CI sobre c1f0e2b7 es el experimento final: breaking-surface debe dar verde en el runner por primera vez.
