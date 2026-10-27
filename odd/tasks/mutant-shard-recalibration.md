# Recalibración de shards de mutants + probes pendientes del review de #2295

Issue: #2296 (`status:approved`, `type:chore`).
Rama: `chore/mutant-shard-recalibration` ·
Worktree: `~/Projects/Rust/webfang-worktrees/chore-mutant-shard-recalibration`

---

## Specs

**S1.** El issue (#2296), verbatim:

> ### Task
> Recalibrar la fórmula de shards del gate de PR (`MUTANT_SHARD_SIZE`/`MUTANT_SHARD_CAP` en
> `scripts/ci_mutant_shards.sh`) con mediciones reales de tiempo de build base y por-mutante,
> y cubrir los dos probes de hardening/fail-closed pendientes del review de #2295: (1) un
> probe para el exit 6 de `cargo mutants --list` (diff inválido, no solo el exit 5 ya
> probado); (2) validar que el conteo derivado de `jq` sea un entero no-negativo antes de la
> aritmética de `emit_matrix`.
>
> ### Acceptance criteria / done when
> - Medición registrada (build base de webfang_core + tiempo por mutante) y
>   fórmula/knobs/timeout actualizados si corresponde, con la evidencia en el log del doc de
>   la tarea.
> - `scripts/test_ci_mutant_shards.sh` incluye un probe que fuerza el exit 6.
> - El conteo de `shards` valida `^[0-9]+$` antes de la aritmética, con su caso de test.
> - `scripts/test_ci_fast_gate.sh` sigue en 69 ok / 0 fail y el harness de shards en 24/24.

**S2.** Contexto del review de #2295 (advisory, verbatim del issue):
- **R4-budget**: con 8 mutantes/shard y timeout de 45 min el margen es el más justo del
  arreglo (~36 min de mutantes + instalación/toolchain vs 45 min); la S1 de #2285 declara la
  fórmula provisional hasta calibrar.
- **R3-3**: el harness solo ejercita el signature exit 5; la rama exit 6 del contrato S7
  (docs cargo-mutants) no tiene probe (`test_ci_mutant_shards.sh`).
- **R3-4**: el conteo entra sin validar a la aritmética (a diferencia del guard `^[0-9]+$`
  de `shard-count`); con el pin de cargo-mutants@27.1.0 es inalcanzable, pero es hardening
  barato.

**S3.** Restricción del owner: cuidado con no saturar los recursos del sistema al ejecutar
comandos pesados. La medición pesada (build base, mutantes) corre en background, con `nice`,
y monitoreada; lo que pueda salir de datos de CI reales (gh) no cuesta recursos locales.

---

## Tasks

- [x] T1 · S1 — probe exit 6 en `scripts/test_ci_mutant_shards.sh` (sonda empírica del
  trigger + caso en el harness).
- [x] T2 · S1 — guard `^[0-9]+$` del conteo de `jq` en `cmd_shards` + caso de test
  (cargo falsificado en PATH que emite JSON válido no-entero).
- [x] T3 · S1, S3 — medición: timings reales de los shards de CI (logs de GitHub) +
  sondeos empíricos de cargo-mutants 27.1.0; el build local frío se descartó por
  redundante y por S3 (ver L3/L4).
- [x] T4 · S1 — decisión de fórmula/knobs/timeout con la evidencia y actualización de
  comentarios (`ci_mutant_shards.sh`, `mutants.yml`) + corrección del defecto de
  indexación de shards descubierto en la medición (ver L5).
- [x] T5 · S1 — verificación: harness de shards 28/28, fast gate, actionlint/shellcheck
  con la lista compartida de CI, commits work-unit.

---

## Log

L1 — Petición original del owner (verbatim):

> Issue 2296 analizar y revisar luego implementar, pero ten cuidado en saturar los recursos
> del sistema cuando ejecutas comandos pesados, utiliza context7 y jina en caso de necesitar
> investigar alguna duda

L2 — **Medición de CI (la evidencia primaria; hardware real del gate).** Run
40586006186 (PR `chore/infra` tokio::spawn lint, 2026-10-27), job
`cargo-mutants (PR diff shard 1)`, runner ubuntu-24.04 (Azure, 4 vCPU), cargo-mutants
27.1.0, `CARGO_PROFILE_DEV_DEBUG=0`:

- Fase de mutantes: **24m13s para 6 mutantes** ("Found 6 mutants to test" 21:27:42 →
  "6 mutants tested in 24m" 21:51:55): 2 timeouts (55 s+300 s, 64 s+300 s), 2 missed
  (53 s+246 s, 64 s+250 s), 2 unviable.
- Por-mutante: build incremental 53-64 s; test 246-300 s (los mutantes de
  `ChromiumoxideDownloader::fetch` son async y su test toca el techo `--timeout=300`).
  Peor caso por mutante ≈ **61 s + 300 s ≈ 6 min**.
- Overhead no-mutante: wall del job 24m43s menos 22m12s reportados ≈ **2-3.5 min**
  (checkout, toolchain, instalación, build frío de deps plegado en el primer mutante).

L3 — **Aritmética de la decisión.** Peor caso por shard = overhead + tamaño × 6.1 min:

| Tamaño | Peor caso | vs timeout 45 min |
|---|---|---|
| 8 (el provisional) | 3.5 + 8×6.1 ≈ **52 min** | SE PASA — confirmación con datos del R4-budget de #2295 |
| **6 (nuevo)** | 3.5 + 6×6.1 ≈ **40 min** | cabe con ~5 min de margen |

→ `MUTANT_SHARD_SIZE` 8 → **6**; `MUTANT_SHARD_CAP` queda en 16 (techo de costo, no
promesa de sizing: un diff que supere `cap×size` mutantes dibuja `cap/size` por shard y
sus shards pueden tocar el timeout — fail-closed; la válvula es partir el PR).
Timeout de shard queda en 45 min. La S1 de #2285 queda así cerrada: la fórmula ya no es
provisional.

L4 — **Por qué no hay build local frío en la evidencia.** El build base local se descartó
por dos razones: (a) la medición válida para calibrar el gate es la del runner de CI (L2),
no la de una estación de 16 núcleos; (b) el wrapper de cache global (mbx) sirvió el build
desde cache (45.77 s), y medir frío de verdad habría exigido una compilación completa
artificial que viola la restricción S3 del owner. Los sondeos empíricos de cargo-mutants
(S6 abajo) son locales pero no compilan (`--list`).

L5 — **Defecto descubierto por la medición: los shards emitidos eran 1-based.** El shard
2 del run 40586006186 murió a los 42 s con `error: invalid value '2/2' for '--shard
<SHARD>': shard k must be less than n`, enmascarado por `continue-on-error: true` (job
verde, artefacto de 504 bytes). Verificado contra cargo-mutants 27.1.0 (DeepWiki + sondeos
locales): `--shard k/N` exige `0 <= k < N` (k==N es error de CLI) y reparte por índice
(método slice: los primeros `ceil(count/N)` van al shard 0). Consecuencia del `shards=
[1..N]` previo: la partición 0 **nunca se probaba** y el último shard siempre moría —
hueco de cobertura silencioso tanto en el gate de PR como en la matriz weekly 1..96.
Corrección: `emit_matrix` emite `0..N-1`; la matriz weekly pasa a `0..95`; sondeos de
regresión: `--shard 0/2`, `1/2`, `0/96`, `95/96` → rc 0; `2/2`, `96/96` → rc 1.

L6 — **Exit 5 vs exit 6, medido contra 27.1.0** (sondeos en fixture de juguete, sin
compilar). El comentario de #2285 decía "exit 6 = the diff is not a diff"; la realidad es
más fina:

| Entrada | rc |
|---|---|
| diff válido | 0 |
| archivo vacío / texto plano / header sin hunks / hunk con conteo inflado / binario crudo | 0 (diff "vacío") |
| new side que no matchea el árbol / hunks malformados (`@@ garbage @@`) / unified sin `diff --git` | **5** |
| archivo diff inexistente / es un directorio / `chmod 000` (EACCES) | **6** |

→ exit 5 = diff legible pero inutilizable; exit 6 = el PATH del diff no se puede abrir.
El probe del harness usa `chmod 000` (determinista, pasa el pre-check `-f` del helper y
hace que cargo reciba EACCES; nunca corre como root).

L7 — **Guard del conteo jq (R3-4).** `jq 'length'` sobre un número devuelve su valor
absoluto: un documento JSON `3.7` produce el conteo no-entero `3.7` que la aritmética de
`emit_matrix` no puede tragar. El probe falsifica `cargo` en PATH (stub que emite `3.7`):
el guard `^[0-9]+$` debe fallar con diagnóstico nombrado; y el control (el mismo stub
emitiendo `["m1","m2"]`) debe seguir dimensionando la matriz — el guard está delante de
la aritmética, no delante de cargo.

L8 — Verificación: harness de shards **28/28** (era 24/24; +1 exit 6, +2 guard, +1
0-based, +1 caso de fórmula por el nuevo tamaño), actionlint + shellcheck limpios con la
lista compartida de CI (`SC2010 SC2012 SC2027 SC2034 SC2046 SC2086 SC2126 SC2129`), YAML
válido. Nota de entorno local: el wrapper `cargo` de mise re-resuelve el entorno por CWD,
así que el harness corre con el binario real de cargo-mutants 27.1.0 en PATH (los
fixtures viven en /tmp, fuera de la config de mise); en CI no aplica.
