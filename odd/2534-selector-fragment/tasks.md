# Selector explícito sin Readability — Tasks

Spec: #2534 (`--selector` deja de pasar por legible: el fragmento elegido se serializa
directo a Markdown). Rama `feat/2534-selector-fragment`.

Decisiones cerradas antes de escribir código:

- El comportamiento vive **dentro de** `extract_with_selector`, no en una función paralela:
  `adaptive_selector_repair` (`extraction.rs:211`) vuelve a entrar por ahí, y un selector
  reparado tiene que heredar la misma ruta.
- `ExtractResult::Matched` pasa a llevar el fragmento final, no el envoltorio
  `div#selector-extracted`.
- `legible 0.4.3` no soporta `<base href>` (cero referencias a `Base` en el crate), así que
  resolver contra la URL de la página reproduce el comportamiento actual: no hay regresión.
- `htmd::convert` usa `parse_document` (`htmd-0.5.5/src/lib.rs:113`), así que un `<tr>`
  suelto pierde sus etiquetas al convertir: el wrapper `<table><tbody>` pertenece a la
  serialización, no al fixture.
- `result_size_factor` (`structural_score.rs:75-80`) puntúa `as_html().len()`, pero sus tests
  solo construyen `Fallback`: no hay snapshots que se muevan.

- [x] T1 — `serialize_matches` + `absolutize_urls` en `application/extraction.rs`: dedup de
  ancestros por `ancestors()` + `id()`, separador `\n\n` entre bloques, absolutización de
  `a[href]`/`img[src]`/`source[src]`/`video[poster]` con `lol_html`, y `None` con cero
  matches. Prueba: tabla HTML en el módulo (`nested_div_emitted_once`,
  `relative_href_and_img_src`, `unresolvable_schemes_untouched`,
  `two_siblings_joined_by_blank_line`, `no_match_is_none`).
- [x] T2 — tabla Markdown por el mismo `HtmdConverter` que usa `file_saver.rs:180`:
  `h3`, `table`, y las filas de caracterización `li`, `tr` (dentro de `<table><tbody>`),
  `td` (dentro de tabla) y `strong`. Una sola corrida mide todas: el bucle acumula fallos.
- [x] T3 — cerrado con datos. El wrapper `<table><tbody>` se implementó en
  `serialize_matches` para la familia `tr`/`td`/`th`/`thead`/`tbody`/`tfoot` (`caption`
  queda fuera: pertenece fuera de `<tbody>`). Medición: sin wrapper `--selector tr` daba
  `ab` (frontera de celda perdida); con wrapper da `a\n\nb`. Sigue **sin ser tabla**: htmd
  necesita un `<thead>` para construir la fila de cabecera. Limitación documentada en la
  fila `characterize_tr_inside_table`; `--selector table` sí produce tabla GFM real.
- [ ] T4 — commit mecánico: añadir `base_url` a la firma de `extract_with_selector` sin usarlo,
  actualizar los 24 call sites y el fuzz target. Snapshots sin cambios.
- [ ] T5 — commit de comportamiento: `Matched` lleva el fragmento, dedup, absolutización, y los
  dos tests del envoltorio (`extraction.rs`, `scraper_service_test.rs`).
- [ ] T6 — conectar las dos rutas (`extract_content` y `scrape_page`/`build_scraped_content`),
  eximir el guard de 50 caracteres en la ruta con selector y regenerar solo los snapshots con
  selector.
