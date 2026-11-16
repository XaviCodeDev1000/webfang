# Miri advisory report filter — `fix/miri-advisory-report-filter`

Objetivo: que el reporte de jobs Miri advisory en `ci.yml` no abra trackers fantasma con jobs en curso, y que no duplique issues cuando Miri falla de verdad.
Estrategia: `single-pr` · Runner: validación `jq` contra JSON real de la API + `actionlint`/`shellcheck` del workflow + run de verificación.

## Specs

- **S1** (texto literal del usuario): "PR del filtro de ci.yml. Valida el jq nuevo contra JSON real antes de abrir el PR: Con un snapshot de la API con jobs in_progress (conclusion: null), el resultado debe ser lista vacía. Con el run final, debe listar solo Miri (core). Pega ambas salidas en la descripción del PR."
- **S2** (texto literal del usuario): "Filtro de #2836: select(.conclusion == \"failure\") sirve, pero considera también timed_out y cancelled. Mejor: select(.status == \"completed\" and (.conclusion | IN(\"success\",\"skipped\",\"neutral\") | not)). Además, mueve el reporte a un job con needs: [los Miri] y if: always(), para que no corra con jobs en curso."
- **S3** (texto literal del usuario): "Issue de sigaction. Ábrelo, cita #2671 como precedente del patrón de gate/inyección y aclara que es otra clase de problema (syscall no soportada, no UB de aliasing). Enlaza #2723 solo si realmente hay relación, para no contaminar la deuda de gates con algo distinto."
- **S4** (texto literal del usuario): "#2836: ciérralo como falso positivo, pero enlazando al PR del filtro y con un comentario que incluya la línea de tiempo corregida. Así queda documentado por qué ocurrió. #2837 sigue abierto hasta que el run quede verde."
- **S5** (texto literal del usuario): "Tras mergear el fix del filtro, comprueba en el siguiente run programado que no se vuelve a crear un tracker fantasma. Si el reporte pasa a un job con needs + if: always(), confirma también que no se duplican issues cuando Miri falla de verdad (que actualice el existente en vez de abrir uno nuevo)."
- **S6** (hecho verificado): el run `43804159513` arrancó `04:26:57Z`, #2836 se creó `04:36:01Z` (+9m04s), el run terminó `05:42:32Z` (1h15m35s) y #2837 se creó `05:42:47Z` (+15s). A los 9 minutos los seis jobs Miri tenían `conclusion: null`.

## Tasks

- **T1** (S1, S2, S6) — inline — reemplazar el jq del paso `miri-report` por el filtro con `status == "completed"`; mover el reporte a `notify` con `needs` de `miri` + `miri-infra-slow` (los seis jobs del matrix) e `if: always()`; validar ambas salidas contra JSON real. **Hecho**. Evidencia jq (expresión extraída del propio workflow): snapshot `in_progress` → `[]`; run final `43804159513` → `Miri (core)`. Gate local CI-only: 18 PASS / 0 FAIL / 18 SKIP, `actionlint` PASS. Commit: el presente commit.
- **T2** (S3) — inline — abrir issue de `sigaction`, citando #2671, sin enlazar #2723. Commit: N/A (operación de GitHub).
- **T3** (S4) — inline — cerrar #2836 con comentario de timeline y enlace al PR; dejar #2837 abierto. Commit: N/A (operación de GitHub).
- **T4** (S5) — inline — verificación posterior al merge en el próximo run `schedule`: sin tracker fantasma y sin duplicado cuando Miri falla de verdad. Commit: N/A (observación).

## Log

**L1** ( petición original del usuario, verbatim ):

> PR del filtro de ci.yml. Valida el jq nuevo contra JSON real antes de abrir el PR:
>     Con un snapshot de la API con jobs in_progress (conclusion: null), el resultado debe ser lista vacía.
>     Con el run final, debe listar solo Miri (core).
>     Pega ambas salidas en la descripción del PR.
>
> Filtro de #2836: select(.conclusion == "failure") sirve, pero considera también timed_out y cancelled. Mejor: select(.status == "completed" and (.conclusion | IN("success","skipped","neutral") | not)). Además, mueve el reporte a un job con needs: [los Miri] y if: always(), para que no corra con jobs en curso.
>
> Issue de sigaction. Ábrelo, cita #2671 como precedente del patrón de gate/inyección y aclara que es otra clase de problema (syscall no soportada, no UB de aliasing). Enlaza #2723 solo si realmente hay relación, para no contaminar la deuda de gates con algo distinto.
>
> #2836: ciérralo como falso positivo, pero enlazando al PR del filtro y con un comentario que incluya la línea de tiempo corregida. Así queda documentado por qué ocurrió. #2837 sigue abierto hasta que el run quede verde.

**L2** ( corrección de redacción del agente, 2026-11-16 ): el agente había escrito "unas 5 horas antes"; la diferencia real entre la creación de #2836 y el fin del run es 1h06m31s. Verificado con timestamps de la API.
