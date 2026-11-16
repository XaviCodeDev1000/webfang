# Cross-platform error snapshots — `fix/cross-platform-error-snapshots`

Objetivo: hacer que los snapshots de error DNS/conexión sean idénticos en Linux, macOS y Windows, sin hornear variantes por plataforma.
Estrategia: `single-pr` · Runner: `cargo nextest run -p webfang_core --test dns_tls_exit_code_test`, `cargo test -p webfang_test_utils`, gate completo antes del PR.

## Specs

- **S1** (texto literal del usuario): "PR de snapshots/regex (macOS + Windows). Sigue las condiciones anteriores: arregla el regex en sí y evita hornear snapshots específicos de plataforma. Antes de abrirlo, dime qué haces con #2835: ¿lo absorbes, lo rebaseas encima o lo dejas como está? No quiero PRs pisándose."
- **S2** (texto literal del usuario): "macOS: arregla el regex en sí, no solo el snapshot. Que (DNS error: )[^,\]\n]* no se corte en la coma. Revisa también si el PR #2835 queda redundante o en conflicto, y coordínalo en vez de duplicar trabajo."
- **S3** (texto literal del usuario): "Windows: en lugar de hornear otro snapshot específico de plataforma, extiende redact_nondeterministic (#1631) para normalizar la diferencia connect_refused / connect_timeout, o haz el assert tolerante a ambas variantes. Los snapshots deben generarse de forma que pasen en Linux, macOS y Windows."
- **S4** (texto literal del usuario): "Antes de pedir review, confirma que el run completo queda verde (salvo Miri advisory) en las tres plataformas, con el enlace al run en el PR."
- **S5** (decisión del agente, comunicada al usuario antes de abrir el PR): dejar #2835 como está; este PR **no toca** `dns_tls_exit_code_test__dns_failure_redacted_stderr.snap`, arregla solo el regex y añade tests. #2835 se coordina con un comentario, no se absorbe ni se rebasea encima.

## Tasks

- **T1** (S1, S2, S5) — inline — arreglar `redact_resolver_payload` para consumir el payload completo hasta `, url: `, `]` o ` at `, sin lookahead (el crate `regex` no lo soporta); añadir tests unitarios con muestras Linux/macOS/WARN de #2835. Commit: pendiente.
- **T2** (S3) — inline — extender `redact_nondeterministic` para colapsar el wrapper `I/O error: el servidor rechazó la conexión:` y normalizar `connect_refused`/`connect_timeout` a un token único; actualizar los snapshots afectados y añadir tests del redactor. Commit: pendiente.
- **T3** (S4) — inline — gate local, push, PR con el enlace al run de verificación de las tres plataformas. Commit: N/A ( entrega ).

## Log

**L1** ( petición original del usuario, verbatim ):

> Perfecto, aclarado. Arranca con todo, con estos matices:
>
> Orden y alcance
>
>     PR de snapshots/regex (macOS + Windows). Sigue las condiciones anteriores: arregla el regex en sí y evita hornear snapshots específicos de plataforma. Antes de abrirlo, dime qué haces con #2835: ¿lo absorbes, lo rebaseas encima o lo dejas como está? No quiero PRs pisándose.
>
>     macOS: arregla el regex en sí, no solo el snapshot. Que (DNS error: )[^,\]\n]* no se corte en la coma. Revisa también si el PR #2835 queda redundante o en conflicto, y coordínalo en vez de duplicar trabajo.
>
>     Windows: en lugar de hornear otro snapshot específico de plataforma, extiende redact_nondeterministic (#1631) para normalizar la diferencia connect_refused / connect_timeout, o haz el assert tolerante a ambas variantes. Los snapshots deben generarse de forma que pasen en Linux, macOS y Windows.
>
> Una cosa más
> Tras mergear el fix del filtro, comprueba en el siguiente run programado que no se vuelve a crear un tracker fantasma. Si el reporte pasa a un job con needs + if: always(), confirma también que no se duplican issues cuando Miri falla de verdad (que actualice el existente en vez de abrir uno nuevo).
>
> Avísame cuando estén los PRs abiertos, con los enlaces al run de verificación.

**L2** ( decisión de coordinación, 2026-11-16 ): #2835 se deja intacto. Su diff sobre el snapshot de DNS añade dos líneas WARN del retry budget; el fix del regex no altera la salida en Linux, así que ese snapshot sigue siendo válido tras mergear ambos PRs en cualquier orden. Se comentará en #2835 enlazando este PR.
