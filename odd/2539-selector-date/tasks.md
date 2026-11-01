# Fecha en la ruta del selector — Tasks

Spec: #2539 (la ruta con `--selector` deja de perder `date` al saltarse legible).
Rama `feat/2539-selector-date`, apilada sobre `feat/2534-selector-fragment` (#2541).

- [x] T1 — `date_extractor` en `domain/html_extraction/`, con la misma forma que
  `author_extractor` (trait + estrategias ordenadas): JSON-LD `datePublished` (siguiendo
  `@graph`) → meta (`article:published_time`, `og:published_time`, `name="date"`,
  `itemprop`) → microdata `[itemprop="datePublished"]` → `article time[datetime]`.
  Prueba: tabla de casos (hoy 22, repartidos en tres funciones por familia), incluido
  `stray_time_outside_article_is_ignored`.
- [x] T2 — conectado en las dos rutas del selector, leyendo el documento **crudo**:
  `clean_html` elimina `<script>`, así que el JSON-LD no sobrevive al limpio y la primera
  estrategia nunca encontraría nada.
- [x] T3 — formato: la cadena de origen recortada, igual que `published_time` de legible
  (que tampoco normaliza). El frontmatter no gana un segundo estilo de fecha.
- [x] T4 — ruta de página completa intacta: `article.published_time` sigue siendo la
  fuente ahí. Ningún snapshot de página completa se movió.
- [x] T5 — descripción de `--selector` actualizada (es la que leen los clientes MCP) y
  README. Cuatro snapshots: los dos de `--help` y los dos del schema MCP.
- [x] T6 — `cargo nextest run --all-features`: 5126 tests, clippy limpio.
- [x] T7 — paridad con legible: `date_cascade_agrees_with_legible_on_shared_sources`
  compara, fixture por fixture, contra `article.published_time`. No es una aserción de
  paridad total: la cascada es más amplia a propósito, y encontrar más de lo que legible
  encuentra no puede hacer que las dos rutas discrepen (del otro lado no hay nada).
- [x] T8 — divergencias aceptadas, fijadas en `documented_divergences_from_legible`: el
  bloque JSON-LD sin `datePublished` no corta la búsqueda (legible hace `break`), y
  `og:published_time`, `name="date"` y `article time` son fuentes que legible no lee.
  La prueba exige las dos cosas: que nosotros encontremos la fecha **y** que legible
  siga sin encontrarla. Si legible empieza a encontrarla, el caso deja de ser una
  divergencia y la prueba lo dice.

## La puerta de JSON-LD

La primera versión aceptaba cualquier bloque con `datePublished`. La prueba de paridad
la rechazó con tres fixtures, y tenía razón: legible exige `@context` de schema.org y un
`@type` de artículo (`Article`, `NewsArticle`, `BlogPosting`, … 19 tipos) antes de
confiar en un bloque, resuelve `@graph` sólo cuando falta `@type`, y en un array toma el
primer elemento con tipo de artículo. Copiar esa puerta es lo que evita que la misma
página muestre una fecha con `--selector` y otra sin él: si somos más laxos, la ruta del
selector publica una fecha donde la ruta completa no ve ninguna.

Los tres fixtures que fallaron no eran un defecto del código sino de la tabla: seguían
sin `@context`/`@type`, describiendo la puerta laxa que ya no existe.

## La tabla partida en tres

La tabla única tenía 22 casos y 128 líneas, y clippy la rechazaba por `too_many_lines`
(100). Partirla sin más habría dejado la misma duplicación, así que las dos cosas se
resolvieron a la vez: cada fila repetía el esqueleto del documento
(`<html><head>…</head><body></body></html>`) y el bloque de acumulación de fallos.

- `page(head, body)`, `jsonld(payload)`, `meta(attr, value, content)` y
  `article_jsonld(date)` arman el fixture; la fila escribe sólo lo que distingue al caso.
- `assert_cases` corre la tabla y `assert_no_failures` reporta todas las filas que fallan,
  no la primera.
- Partición por familia, no por tamaño: `jsonld_article_blocks_supply_the_date` (7 filas),
  `jsonld_blocks_outside_the_gate_are_ignored` (8) y
  `meta_microdata_and_article_time_supply_the_date` (7). Cuando una falla, el nombre dice
  de qué familia es. Ninguna llega a 100 líneas, y no hay `#[allow(clippy::too_many_lines)]`.

El otro clon estaba entre este módulo y `author_extractor`: los dos definían su propio
`SEL_JSONLD`, `non_empty` y `text_content`. Se mudaron al módulo padre
`html_extraction`, que es el único sitio del que los dos hijos ya dependían. El ratchet
de duplicación quedó en **8315** contra el baseline de 8325.

## Verificación cruzada con el bump de legible (#2543)

La paridad se mide contra la versión de legible que esté en el lock, y
`documented_divergences_from_legible` exige que legible **siga** sin encontrar esas
fechas. #2543 sube a 0.5.1, que reescribió su árbol interno (`Node<'_>` → arena
`Dom` + `NodeId`). Cada PR pasa verde sola; la combinación no se había probado nunca, y
un cambio en el tratamiento del JSON-LD sólo se vería con las dos juntas.

Fusión desechable. `chore/legible-0.5` ya está apilada sobre #2541, así que mergear
#2548 encima da el estado exacto que tendrá `main` tras los tres merges:

    git worktree add --detach /tmp/merge-probe chore/legible-0.5
    cd /tmp/merge-probe && git merge --no-edit feat/2539-selector-date
    cargo nextest run --all-features

Resultado con legible 0.5.1: **5126 tests, 0 fallos**. Ninguna divergencia documentada se
invirtió, así que no hubo expectativa que actualizar. Hay que repetirlo después de mergear
#2541 y antes de marcar la segunda PR como lista.

Dato que sale de esto: 0.4.3 implementa la puerta como regex y 0.5.1 como lista ordenada
con `binary_search`. Los fixtures pasan con las dos, o sea que no distinguen esa
diferencia — por eso la lista quedó anotada en el comentario del código con la versión de
origen.

## Mutaciones

Cada rama de la puerta tiene una fila que la mata. Medido el 2026-11-01 con la tabla ya
partida en tres funciones, para que la evidencia corresponda al código que se entrega:

| mutación | prueba que falla |
|---|---|
| quitar el chequeo de `@context` | `jsonld_blocks_outside_the_gate_are_ignored` (`jsonld_without_context_is_ignored_even_when_article_typed`, `jsonld_graph_without_context_is_ignored`) y la de paridad |
| aceptar cualquier `@type` como artículo | `jsonld_blocks_outside_the_gate_are_ignored`, `jsonld_article_blocks_supply_the_date` y la de paridad |
| que `@graph` tome el primer elemento | `jsonld_article_blocks_supply_the_date` (`jsonld_graph_array`), `jsonld_blocks_outside_the_gate_are_ignored` y la de paridad |
| caer a `dateModified` | `jsonld_blocks_outside_the_gate_are_ignored` (`date_modified_is_not_a_publish_date`) |
| aceptar cualquier `<time datetime>` sin `<article>` | `meta_microdata_and_article_time_supply_the_date` (`stray_time_outside_article_is_ignored`) |

Las dos primeras familias se pisan a propósito en las mutaciones de la puerta (una fila
positiva también cae cuando la puerta se abre), pero `dateModified` y el `<time>` suelto
matan **sólo** a su propia familia: la partición no es decorativa.

La prueba de paridad no es vacía: con la puerta laxa vuelve a fallar en los dos fixtures
sin `@context`.

## `cargo mutants --in-diff` no cubre este código

`cargo-mutants (PR diff)` es un check requerido, pero su alcance lo fija
`.cargo/mutants.toml` (`examine_globs`) y ahí no está `domain/html_extraction/**` ni
`application/scraper_service.rs`. Medido sobre el diff de esta PR:

    cargo mutants -p webfang_core --list --json --in-diff <diff>   # 0 mutantes
    cargo mutants -p webfang_core --list --json                    # 1485 mutantes, 39 ficheros

Ninguno de los 39 ficheros mutados es de este cambio, así que el job mide cero mutantes y
el check pasa sin analizar nada. No es un fallo de esta PR: el alcance es una decisión
anterior (#1235, #1825) y ampliarlo tiene coste semanal. Las mutaciones de arriba son
manuales porque la automática no llega; queda anotado para decidir aparte si
`html_extraction` merece entrar en `examine_globs`.

## Auditoría de `preferred_title` (#2552)

#2541 unificó el fallback de título en `preferred_title(original_title, fallback)`.
Antes del cambio había **dos** copias, no tres, y eran idénticas:

    if original_title.is_empty() { &article.title } else { original_title }   // ruta legible
    if original_title.is_empty() { &fallback_title } else { original_title }  // ruta fallback

La tercera es la ruta nueva del selector, que copia la forma de la segunda
(`url.host_str().unwrap_or("unknown_host")`). El helper no igualó dos variantes distintas:
la divergencia `host` vs `Document: [host]` de #2552 vive en `resolve_title`, aguas abajo,
y sigue igual. Evidencia: ningún `.snap` se movió por este helper en #2541.

## Nota

La descripción que viaja al schema MCP se mantuvo corta a propósito: cada cliente la
carga en contexto en cada llamada. El detalle vive en el README.
