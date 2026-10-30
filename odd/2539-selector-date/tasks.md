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
- [x] T6 — `cargo nextest run --all-features`: 5124 tests, clippy limpio.

## Nota

La descripción que viaja al schema MCP se mantuvo corta a propósito: cada cliente la
carga en contexto en cada llamada. El detalle vive en el README.
