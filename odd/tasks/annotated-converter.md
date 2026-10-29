# Mission: annotated-converter (Opción 3, slice B)

Branch: `feat/annotated-converter` · Worktree: `feat-annotated-converter` · Base: `origin/main` @ 91d416c3
(Slice A de esta Opción — locators en search results — vive en `feat-vault-locator-results` @ 1f4e19c6, PR propio.)

## Specs

- S1 (verbatim, misión anterior): "Diseña pensando en la Opción 3: Cuando hagas la
  conversión de HTML a Markdown, no lo hagas a ciegas. Si usas una librería de parseo,
  intenta guardar un metadata básico (ej. número de línea o un ID de bloque HTML) en el
  chunk antes de que entre al chunker."
- S2: sin respuesta del usuario a la pregunta de diseño, se procede por best judgment con
  la opción recomendada: **tool MCP on-demand** — `convert_html_to_markdown` devuelve el
  mapa de bloques junto al markdown, sin storage. Alternativas registradas (ADR): export
  con sidecar `<file>.md.map.json` (deferido), metadata en DocumentChunk (reencuadre: el
  cleaner usa otro extractor, no este conversor).
- S3: hallazgo de mapeo que reencuadra S1: `convert_to_markdown` NO alimenta al chunker
  del scraper — alimenta el export (`file_saver`) y la tool MCP. El "chunk" de S1 se
  interpreta como "el consumidor del markdown convertido".

## Tasks

- T1: ✅ Experimento — `convert_with_visitor` NO disponible (feature `visitor` no default;
  activarla es un gate de permisos) → camino per-block split. Observado: cada pieza
  convierte con trailing `\n`; whole-doc termina en `\n` de documento.
- T2: ✅ RED — invariante del mapa (compile RED E0425 + 1 fallo funcional: el `\n`
  final del documento no es de ningún bloque → se agrega tras el último span).
- T3: ✅ GREEN — `AnnotatedMarkdown { markdown, blocks }` + `MarkdownBlock
  { markdown_span: ByteSpan, html_tag, html_index }` + `convert_to_markdown_annotated`
  (scanner top-level con quotes/comentarios/void, unwrap html/body, invariante
  join==whole con degradación honesta + WARN). `convert_to_markdown` intacto (15 callers,
  mismo output byte a byte).
- T4: ✅ GREEN — tool MCP `convert_html_to_markdown` devuelve JSON estructurado
  `{markdown, blocks}` (provenance wrap intacto); helper `links_tool_result` generalizado
  a `json_tool_result` (mensaje de fallo por tool); 2 tests nuevos de handler.
- T5: ✅ Gate — 5044/5044 workspace, clippy --all-features 0, fmt OK, rustdoc exit 0.
- T6: Commits por work-unit + este log.

## Risk

- Medio: nueva API pública en domain/converter + cambio aditivo del output de una tool
  MCP. Superficie: `infrastructure/converter/html_to_markdown.rs` (+ helpers),
  `crates/webfang_mcp/src/mcp_server/handlers/content.rs`. `convert_to_markdown` (15
  callers) NO cambia su firma ni su output. NO tocar: `docs/`, `CHANGELOG.md`, `.github/`.

## Log

- L1: Diseño de la Opción 3 aprobado en la misión anterior (chunk-locators).
- L2: Pregunta de diseño sin respuesta → best judgment: tool on-demand (recomendada);
  alternativas (sidecar de export, metadata en DocumentChunk) quedan en el ADR.
- L3: Experimento visitor: semántica `visit_element_end(ctx, element_content)` correcta,
  pero feature-gated → per-block split sin tocar Cargo.toml.
- L4: Invariante fix: el `\n` final del whole-doc es formato del documento — se replica
  fuera del último span. Con eso el mapa es EXACTO para HTML por bloques.
- L5: Degradación honesta verificada: texto suelto + inline cruzando elementos →
  `blocks: []` + markdown byte-exacto. El mapa jamás es incorrecto, a lo sumo ausente.
