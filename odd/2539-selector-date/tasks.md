# Fecha en la ruta del selector — Tasks

Spec: #2539 (la ruta con `--selector` deja de perder `date` al saltarse legible).
Rama `feat/2539-selector-date`, apilada sobre `feat/2534-selector-fragment` (#2541).

- [x] T1 — `date_extractor` en `domain/html_extraction/`, con la misma forma que
  `author_extractor` (trait + estrategias ordenadas): JSON-LD `datePublished` (siguiendo
  `@graph`) → meta (`article:published_time`, `og:published_time`, `name="date"`,
  `itemprop`) → microdata `[itemprop="datePublished"]` → `article time[datetime]`.
  Prueba: tabla de 10 casos, incluido `stray_time_outside_article_is_ignored`.
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

## Mutaciones

Cada rama de la puerta tiene una fila que la mata:

| mutación | fila que falla |
|---|---|
| quitar el chequeo de `@context` | `jsonld_without_context_is_ignored_even_when_article_typed`, `jsonld_graph_without_context_is_ignored` |
| aceptar cualquier `@type` como artículo | `jsonld_non_article_type_is_ignored` |
| que `@graph` acepte cualquier `@type` | `jsonld_graph_array`, `jsonld_graph_without_article_type_is_ignored` |
| caer a `dateModified` | `date_modified_is_not_a_publish_date` |

Y la prueba de paridad no es vacía: con la puerta laxa vuelve a fallar en los dos
fixtures sin `@context`.

## Nota

La descripción que viaja al schema MCP se mantuvo corta a propósito: cada cliente la
carga en contexto en cada llamada. El detalle vive en el README.
