# Ola 0 — batch 4: cierre de la ola por excepciones declaradas (paraguas #2584)

Objetivo: formalizar en el ratchet los tres archivos restantes de la banda [501, 570] como
excepciones declaradas (`file_size_rust.exceptions`), cerrando Ola 0. Branch:
`refactor/ola0-batch4` (pre-construida sobre `refactor/ola0-batch3` @ 10118734; su PR se abre
recién después del merge de #2618, rebasada sobre el nuevo `main` — entrega secuencial).
Ratchet: `scripts/check_file_size.sh` + harness `scripts/test_file_size.sh`.

## Specs

S1 (alcance): "Secuencial pre-build" — decisión del usuario: batch 4 = declaraciones de
excepción (no splits), pre-construido en worktree propio mientras #2618 está en CI; su PR es
propio, separado del del batch 3.
S2 (mecánica): "crear una nueva rama dentro del worktree" corregido por política: un worktree
por sesión/rama — el batch 4 vive en su segundo worktree, con rama propia basada en la del
batch 3; la fusión al árbol público es vía PR propio tras el rebase, no vía merge local al PR
del batch 3.
S3 (excepciones): "quedan grandfathered con el porqué en las notes del JSON. La decisión se
toma POR archivo en su misión" — las tres rationales (provenance, handlers/ai, windows_console)
viajan en el PR para la decisión del maintainer (exploración del batch 3 como evidencia).
S4 (baseline): "el baseline solo baja" — las annotations NO tocan conteos (diff: +5 líneas, un
mapa hermano `exceptions` invisible para el parser del guard).
S5 (entrega): push y PR solo con confirmación explícita del usuario; PR con `Closes part of
#2584`, label `type:refactor`, base `main`, recién tras el rebase post-merge de #2618.
S6 (método): el método completo vive en `odd/tasks/file-size-reduction.md` — este registro no
lo reescribe.

## Tasks

| ID | Tarea | Specs | Ruta | Estado | Commit |
|----|-------|-------|------|--------|--------|
| T1 | worktree propio + bootstrap (rama desde ola0-batch3) | S1,S2 | inline | done | — |
| T2 | diseño del mecanismo de notas: mapa `exceptions` hermano (parser-safe) | S3,S4 | inline | done | d8951984 |
| T3 | verificar: guard verde, harness 19/19, queries de banda intactas | S4 | inline | done | d8951984 |
| T4 | registro de misión + espejo Engram | S6 | inline | done | — |
| T5 | tras merge de #2618: rebase sobre `main`, push + PR propio (confirmación) | S5 | inline | todo | — |

## Log

L1 — Pedido original (verbatim, 2026-11-06): "podemos crear ua nueva rama dentro del worktree
implementar el batch 4 y luego fusionar antes de mergear ?" — corrección de mecánica (rama =
worktree nuevo) y decisión de entrega: "Secuencial pre-build (recomendado)".
L2 — Implementación (2026-11-06): mapa `file_size_rust.exceptions` con las tres rationales
(d8951984); guard verde (grandfathered 101 sin cambios), `test_file_size.sh` 19/19, query de
banda devuelve exactamente los 3 archivos restantes. El pre-build queda local hasta el merge
de #2618.
