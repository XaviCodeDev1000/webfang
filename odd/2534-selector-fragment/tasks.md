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
- [x] T4 — commit mecánico: añadir `base_url` a la firma de `extract_with_selector` sin usarlo,
  actualizar los 24 call sites y el fuzz target. Snapshots sin cambios.
- [x] T5 — commit de comportamiento: `Matched` lleva el fragmento, dedup, absolutización, y los
  dos tests del envoltorio (`extraction.rs`, `scraper_service_test.rs`).
- [x] T6 — conectar las dos rutas (`extract_content` y `scrape_page`/`build_scraped_content`),
  eximir el guard de 50 caracteres en la ruta con selector y regenerar solo los snapshots con
  selector.

## Evidencia de mutación (T1)

Tres mutaciones, tres filas distintas muertas — la tabla discrimina:

- filtro de ancestros roto (`.any(|_| false)`) → falla sólo `nested_div_emitted_once`;
- absolutización desactivada → falla sólo `relative_href_and_img_src`;
- guarda de `#` eliminada → falla sólo `unresolvable_schemes_untouched`. La segunda mutación **no** mata esa fila: es correcto, la fila afirma que los valores quedan intactos.

## T4 verificado

4254 tests en verde, ningún `.snap` modificado, el fuzz target compila (`cargo check` dentro de `fuzz/`, que vive fuera del workspace). La URL base es la misma que recibe legible: `fetch_html` no la reescribe tras una redirección y ambos sitios de producción ya le pasan el mismo `url`. Queda `fuzz/Cargo.lock` sucio (7.1.0 → 7.2.0): es staleness previa del workspace de fuzz, no de este cambio.

## T5: los snapshots NO cambiaron, y está bien

Ni `selector_h3` ni `selector_table` se movieron, por dos razones distintas:

- `selector_h3`: el criterio "sin `excerpt:`" sólo se manifiesta contra legible 0.5.x. Con 0.4.3 el snapshot **ya** salía sin esa línea, así que no había nada que quitar. El criterio queda verificado por construcción: la ruta del selector fija `excerpt: None` sin consultar a legible.
- `selector_table`: el fixture usa `<table><tr><td>` **sin `<thead>`**. htmd necesita cabecera para armar la tabla, así que el resultado nuevo es texto separado por líneas en blanco — idéntico al que Readability producía. El snapshot pasaba por coincidencia, no por corrección.

Por eso se agregó `selector_table_with_header_renders_markdown_table` con `<thead>`: prueba la tabla GFM real (| Region | Revenue |) y la ausencia de `excerpt:`. Es un snapshot **nuevo**; ningún snapshot existente cambió.
