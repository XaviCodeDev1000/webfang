# PRD-MIG-2026-01 · v2 — Migración a Rust 1.100 + edition 2024

| Campo | Valor |
|---|---|
| ID | `PRD-MIG-2026-01` |
| Versión | **2.0** — corregido y re-auditado (sustituye a `PRD-v1.md`, que queda como evidencia) |
| Fecha | 2026-11-16 |
| Estado | **Aprobable** — pendiente de las decisiones de §7, todas en F0 |
| Baseline F0 | `{{BASELINE.head_sha}}` ({{BASELINE.head_date}}) · workspace `{{BASELINE.workspace_version}}` |
| Evidencia | `baseline.json` · `AUDIT-2026-11-16.md` · `TRAIN_OPEN` |
| Contrato | `scripts/audit_migration_baseline.py` · `scripts/check_migration_evidence.sh` |

> **Regla de lectura.** Este documento **no contiene hechos del repo**. Toda cifra,
> versión o línea de soporte es un placeholder `{{BASELINE.*}}` y se resuelve contra
> `baseline.json`, generado por el auditor sobre el SHA de F0. Las decisiones
> de versionado, backports, Miri y edición se **derivan** de `Cargo.toml`,
> `rust-toolchain.toml`, `.github/support.json` y `README.md` en cada corrida.
> Si el repo se mueve, `repo-guards` pone rojo el gate: no se edita una cifra a
> mano, se re-corre `--write` y se re-aprueba §7. Ver §8.

---

## 0. Veredicto y cambio de método

La auditoría externa era correcta en el diagnóstico y se quedaba corta en la
cura. Su receta —«re-corré el audit, renumerá §3, reescribí RF-23/RF-24»— es
literalmente lo que el v1 ya prescribía y lo que falló. Entre el baseline del
v1 y hoy se publicaron tres versiones; la propia revisión midió
`{{BASELINE.support.stable}}.1` cuando el workspace ya iba por
`{{BASELINE.workspace_version}}`. **La auditoría también era una foto.**

El defecto no es el SHA viejo: es un PRD con ~60 constantes copiadas en prosa
en un repo que rota líneas de soporte por release. Repetir la foto reproduce el
bug. La corrección estructural, aplicada aquí, es:

1. **Quitar los hechos del documento** (`{{BASELINE.*}}`).
2. **Derivar las decisiones** de las políticas vivas, en vez de escribirlas
   (`decisions.*` en `baseline.json`).
3. **Convertir la deriva en un gate fail-closed**, no en una nota al pie.

Se mantiene el esqueleto del v1 — toolchain → edición → superficie, lotes
pequeños, tags por fase, ratchets descendentes — porque es correcto.

---

## 1. Re-auditoría F0 (ejecutada, no estimada)

| Hecho | Valor vivo | Fuente |
|---|---|---|
| Workspace / MSRV / canal | `{{BASELINE.workspace_version}}` · `{{BASELINE.toolchain.rust_version}}` · `{{BASELINE.toolchain.channel}}` | `Cargo.toml`, `rust-toolchain.toml`, `mise.toml` (`{{BASELINE.toolchain.mise_rust}}`) |
| Edición | `{{BASELINE.edition.workspace}}` heredada por `{{BASELINE.edition.members_inherit_count}}` miembros; overrides: `{{BASELINE.edition.member_overrides}}` | `crates/*/Cargo.toml` |
| `unsafe_code` | `{{BASELINE.lints.unsafe_code}}` | `[workspace.lints.rust]` |
| Distribución | `{{BASELINE.distribution.kind}}`, crates.io = `{{BASELINE.distribution.crates_io}}` | `README.md` |
| Soporte | STABLE `{{BASELINE.support.stable}}` (`{{BASELINE.support.stable_latest}}`) · MAINTENANCE `{{BASELINE.support.maintenance}}` · primera EOL `{{BASELINE.support.eol_first}}` | `.github/support.json` → `SUPPORT.md` |
| Rotación | `{{BASELINE.support.rotation}}` | `.github/support.json` |
| Miri | `{{BASELINE.miri.policy}}`, `{{BASELINE.miri.lanes}}` lanes, nunca merge gate | `README.md`, `ci.yml`, `sanitizers.yml` |
| Pins de toolchain | `{{BASELINE.toolchain_pins.dtolnay_steps}}` pasos con SHA completo (`{{BASELINE.toolchain_pins.dtolnay_references}}` referencias en total); `{{BASELINE.toolchain_pins.stable_explicit_count}}` `toolchain:` estables + `{{BASELINE.toolchain_pins.nightly_explicit_count}}` nightly | `.github/workflows/*.yml` |
| Targets de release | `{{BASELINE.release_targets}}` | `release.yml` |
| Contadores | `assert!(matches!` **{{BASELINE.counts.assert_matches}}** · `lossy(..).into_owned()` **{{BASELINE.counts.lossy_into_owned}}** · mutaciones de env **{{BASELINE.counts.env_mutations}}** · `allow(unsafe_code)` **{{BASELINE.counts.allow_unsafe_code}}** · `if let` **{{BASELINE.counts.if_let}}** · `strip_prefix(..).unwrap_or(..)` **{{BASELINE.counts.strip_prefix_unwrap_or}}** | auditor |
| Arnés | `{{BASELINE.counts.rust_files}}` `.rs` · `{{BASELINE.counts.snapshots}}` `.snap` · `{{BASELINE.counts.trybuild_stderr}}` `.stderr` · `{{BASELINE.counts.workflows}}` workflows | auditor |

### 1.1 Símbolos que la revisión dejó abiertos — cerrados

Ver `AUDIT-2026-11-16.md` §3 para las rutas exactas. Confirmado en este SHA:
`SessionCap::try_admit` y su bucle de prune; `InFlightGuard` (`Drop` puro) y
`HandlerGuard` (`Drop` **con** abort de `JoinHandle` y `join` en el camino
feliz, es decir supervisión); los 3 `toolchain:` estables; `env_set`,
`env_remove`, `EnvGuard` y `env_invariant_scan`; `McpHandler::call_tool` y
`CatchPanicLayer`. Quedan como **hechos de F0**, no como acciones pendientes.

---

## 2. Los siete bloqueantes, resueltos

**B1 — El corte ya se consumió.** `{{BASELINE.support.stable_initial}}` salió el
2026-11-15 por un cambio de superficie, y ya hay dos patches encima. RF-23 y
RF-24 se re-numeran contra `{{BASELINE.decisions.major_target}}` (§4). El marker
de ruptura de la migración no puede reutilizar el major ya publicado; debe
vivir en la superficie que conserva el merge (`breaking_surface_lib.sh`) y
coordinarse con `release-plz`, escritor único de versión y `CHANGELOG.md`.

**B2 — La red de backports se cae sola.** Con STABLE `{{BASELINE.support.stable}}`,
MAINTENANCE `{{BASELINE.support.maintenance}}` y rotación
«third line back from STABLE», publicar `{{BASELINE.decisions.major_target}}`
envía `{{BASELINE.decisions.eol_on_release}}` a EOL ese mismo día. La línea que
sigue en `{{BASELINE.decisions.msrv_compat_toolchain}}`/2021 es
**`{{BASELINE.decisions.backport_line}}`** (MAINTENANCE post-release). RF-24 y
§12 se reescriben contra esa línea, y `SUPPORT.md` se **regenera** con
`scripts/render-support-md.sh` (es una vista, no se edita a mano). Alternativa
si el maintainer quiere conservar otra línea: excepción explícita de soporte,
documentada en ADR-0021, no una suposición.

**B3 — El actor fantasma.** El README declara distribución
`{{BASELINE.distribution.kind}}` y `crates.io = {{BASELINE.distribution.crates_io}}`.
No existe el consumidor de crate con MSRV viejo. El major se justifica por la
**política de superficie de ruptura del repo** (un `fix!` ya lo disparó) y por
quien **compila de fuente** o consume binarios con una cadena de toolchain
fijada. RF-23 reescrito en ese sentido.

**B4 — Miri no es gate.** Política viva: `{{BASELINE.miri.policy}}`,
`{{BASELINE.miri.lanes}}` lanes, `continue-on-error`, scheduled-only, nunca
merge gate, con límites conocidos (`servo_arc`/Tree Borrows, FFI de BoringSSL,
inline asm). Se **reformulan** K9 y los gates F1/F2 a
«{{BASELINE.decisions.miri_gate_wording}}» (`RF-15`, `RF-23`, §6). Promoverlo
a gate exigiría cambiar `ci_plan.sh`, la allowlist de
`test_ci_gate_structure.sh`, `check_advisory_policy.sh`, la política #1510 y el
README: es una decisión de gobernanza, no un checkbox de este tren. Queda
fuera de alcance, con ADR si algún día se quiere.

**B5 — Deriva de numeración §3→§6.** Tabla corregida en §3. El gate ahora
falla si el PRD cita un `RF-xx` que no esté definido en el índice de §3.1,
así que la ambigüedad no puede reaparecer.

**B6 — RF-08: salida de emergencia.** Además del `--precise`, documentar el
bypass real del cooldown (`CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow
cargo update <crate> --precise <ver>`), el hecho de que versiones sin
`pubtime` no se retienen y que `cargo install` ignora el cooldown. Y añadir el
acceptance que **prueba el override**, no solo el bloqueo (§4).

**B7 — Edición por crate.** Los `{{BASELINE.edition.members_inherit_count}}`
miembros heredan `edition.workspace = true` y no hay overrides: el flip es
**`{{BASELINE.decisions.edition_flip}}`**. Además el v1 tenía el orden
invertido: `cargo fix --edition` migra **a la edición declarada en el
manifiesto**, así que correrlo antes de cambiar `[workspace.package]` no hace
nada. RF-13 reescrito: un commit atómico de `edition = "2024"`, luego
`cargo fix --edition --workspace` (y lotes de revisión por crate), y rollback
por **revert del commit**, no por miembro. `fuzz/` y
`tools/intra-crate-lint/` sí tienen edición propia y ahí sí aplica el flip por
crate.

---

## 3. Renumeración corregida y C14

| Corrección v1 | Cita errónea | RF correcto | Acción |
|---|---|---|---|
| C4, C7 (let chains) | RF-16 | **RF-20** | reescrito |
| C5 (`assert_matches!`) | RF-14, «~8 lotes» | **RF-16**, 6 lotes | reescrito |
| C6 (`lossy_owned`) | RF-15 | **RF-17** | reescrito |
| C9 (`pop_front_if`) | RF-17 | **RF-18** | reescrito |
| C10 (`DropGuard`) | RF-18 | **RF-21** | reescrito |
| C14 (guardas de ruta) | RF-24 (backports) | **RF-19** | **absorbido**: RF-19 ya toca `Path::trim_prefix` y los guards; se le añade el acceptance «0 dependencias nuevas, tests de escape». No se crea un RF huérfano |

C16 también se corrige: el ataque a `arrayref` está corroborado públicamente;
la tabla que corrige al informe tenía su propio error. No cambia el plan.

### 3.1 Índice ejecutable de RFs

El gate valida que todo `RF-xx` citado esté en esta tabla.

| RF | Estado | RF | Estado |
|---|---|---|---|
| **RF-01** | vigente (F0) | **RF-14** | vigente (fixtures) |
| **RF-02** | vigente (F0) | **RF-15** | reescrito (Miri advisory) |
| **RF-03** | vigente; pins recalculados en F0 | **RF-16** | vigente; 6 lotes |
| **RF-04** | vigente; autorización acotada | **RF-17** | vigente (higiene) |
| **RF-05** | ampliado (next solver) | **RF-18** | vigente |
| **RF-06** | ampliado (JSON de Cargo) | **RF-19** | ampliado (absorbe C14) |
| **RF-07** | vigente | **RF-20** | vigente (ratchet) |
| **RF-08** | reescrito (override + acceptance) | **RF-21** | vigente (evaluar, no sustituir) |
| **RF-09** | vigente | **RF-22** | vigente; aprobación en F0 |
| **RF-10** | ampliado (coexistencia con machete) | **RF-23** | reescrito (major derivado) |
| **RF-11** | ampliado (seed protegido) | **RF-24** | reescrito (línea derivada) |
| **RF-12** | vigente | **RF-25** | vigente (ADR-0023) |
| **RF-13** | reescrito (flip atómico) | **RF-26** | vigente (épica F4) |

---

## 4. RFs reescritos (texto que entra al pipeline SDD/RDD)

- **RF-05** — añadir al barrido el **next solver de orphanck por defecto** y
  sus posibles errores nuevos de coherencia; cerrar R-14 como **no aplica** con
  evidencia: la matrix de release es `{{BASELINE.release_targets}}` y ningún
  target es ARMv7.
- **RF-06** — la mitigación estructural no es sólo re-medir la poda: migrar
  `seed_publish.sh` a los **mensajes JSON de Cargo** (`--message-format=json`)
  para localizar build-script outputs, porque el layout de build-dir **no es
  interfaz estable**. Mantener el rechazo fail-closed de `build-dir`.
- **RF-08** — salida de emergencia: `CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow
  cargo update <crate> --precise <ver>` con justificación en el PR. Acceptance
  nuevo: un test/runbook que **ejecuta el override** y comprueba que la
  resolución avanza; el bloqueo y el bypass se prueban los dos. Recordar que
  el cooldown no re-audita `Cargo.lock`, no retiene versiones sin `pubtime` y
  no aplica a `cargo install`: `cargo-deny`/`cargo-audit` siguen siendo gate.
- **RF-10** — decisión: **coexistencia**. `cargo::unused_dependencies` no ve
  dev-dependencies y sólo aparece al buildear todos los targets que pueden
  usar la tabla; `cargo-machete` sigue siendo el productor único de verdad.
  Documentar la limitación en el `Cargo.toml` y en
  `scripts/ci_fast_gate_coverage.tsv` antes de declarar la tabla.
- **RF-11 / §12** — el rollback de F1 asume que el seed
  `{{BASELINE.toolchain.channel}}` sobrevive; **protegerlo** del presupuesto de
  caché hasta publicar. Y en MIG-29, normalizar `1.100` vs `1.100.0` antes de
  comparar (`rust-version` es minor, `channel` es parche).
- **RF-13** — ver B7. Aceptación por crate = tests del área; aceptación de fase
  = los 5 miembros + 2 excluidos en 2024 en un solo diff semántico, con el
  reformateo de `style_edition` en commit separado.
- **RF-15 / K9 / gates F1-F2** — sustituir la exigencia de un Miri en verde por
  «{{BASELINE.decisions.miri_gate_wording}}». Concretar: revisar la conclusión
  del último run scheduled (el README ya documenta el comando `gh run list`),
  y que ningún rojo nuevo sea atribuible al bump. Los sanitizers y
  `test_miri_lane_invariants.sh` siguen siendo gates.
- **RF-19** — incorpora C14: reutilizar `adapters::url_path`,
  `crawler_port::filename` y `infrastructure::export::file_exporter`; **cero
  dependencias nuevas** (verificación `cargo-deny`) y test de escape antes de
  tocar cualquier guarda. `trim_prefix` no es intercambiable cuando el
  fallback no es el original.
- **RF-23** — publicar **`{{BASELINE.decisions.major_target}}`** con marker
  canónico `type!:`/`type(scope)!:` en la superficie que conserva el merge
  (matrix de `breaking_surface_lib.sh`: subject con 1 commit, título del PR con
  ≥2, commits con MERGE; `type!(scope):` es malformado y el footer solo no
  alcanza). Rationale: política de superficie de ruptura del repo y
  consumidores que compilan de fuente — **no** consumidores de crates.io.
  Coordinar con `release-plz` y con la rotación de soporte.
- **RF-24** — la línea que recibe backports compilables con
  `{{BASELINE.decisions.msrv_compat_toolchain}}` es
  **`{{BASELINE.decisions.backport_line}}`** (MAINTENANCE tras el release), no
  la que el v1 nombraba. Regla de compatibilidad: nada de let-chains,
  `assert_matches!`, `from_utf8_lossy_owned`, `trim_prefix` ni APIs ≥1.92.
  Acceptance: backport de prueba compilando con ese toolchain,
  `check_support_drift.sh` verde y `SUPPORT.md` regenerado por su renderer.
- **Ratchet (§13.3)** — el script `check_api_surface_ratchet.sh` nace **wired a
  un workflow en el mismo PR**, con harness que lleve marcadores `# NEGATION:`
  y declaración en `scripts/ci_fast_gate_coverage.tsv`; el meta-guard #2056
  (`test_guard_coverage.sh`) falla si falta cualquiera de las tres cosas. Su
  **aprobación se mueve a F0** porque gatea K3/K4 en F3.
- **§8.3** — la fila WASM es asimétrica: el JIT de `wasmtime` también es
  invisible a Miri, igual que FFI. Al hijo MCP se le exigen tres condiciones
  de contención en ADR-0023: **scrubbing del entorno heredado** (secretos),
  **allowlist de egress** (puede saltarse el guard SSRF del host) y **límites
  de recursos**. A favor: el repo ya contiene panics en el borde MCP
  (`McpHandler::call_tool`, `CatchPanicLayer`, #1611) y el unwind es requisito
  de corrección (#1219): el seam tiene precedente interno.
- **§16 / README** — autorización de `docs/` **acotada a los pins** que el bump
  obliga. El README ya retiró el conteo estático de tests y el badge de Miri
  (`check_docs_claims.sh` los tiene en `RETIRED_CLAIMS`); lo único a matizar es
  la frase que dice que Miri «valida» unidades lock-free, para que no se lea
  como gate. No reintroducir ninguna cifra: el propio README manda a leer el
  `Summary` de `nextest`.

---

## 5. Deriva fina detectada al re-auditar

Además de los siete bloqueantes, el v1 y la revisión entrante difieren del árbol
vivo en: referencias `dtolnay` (`{{BASELINE.toolchain_pins.dtolnay_references}}`,
no las que el v1 usó en su acceptance de RF-03), `if let`
(`{{BASELINE.counts.if_let}}`), `strip_prefix(..).unwrap_or(..)`
(`{{BASELINE.counts.strip_prefix_unwrap_or}}`) y `assert!(matches!`
(`{{BASELINE.counts.assert_matches}}`). `RF-03` debe **derivar** el número de
pasos del gate, no fijarlo en un string de aceptación. Detalle completo en
`AUDIT-2026-11-16.md` §4.

**Bloqueante adicional hallado al re-auditar:** `RF-02` no puede firmarse
sobre este SHA. `check_docs_claims.sh` está rojo por dos aristas de producción
que faltan en el diagrama del README, y el meta-guard en modo mutación reporta
siete harnesses rotos. Ambos se reproducen en un árbol prístino de HEAD: son
deuda previa, no efecto de este tren. F0 debe cerrarlos o excluirlos con
evidencia antes de arrancar F1 (detalle en `AUDIT-2026-11-16.md` §5).

`RNF-03` / `test_utils`: el `unsafe` ya pagado se confirma con
`env_invariant_scan`, los `{{BASELINE.counts.allow_unsafe_code}}` allows y el
snippet de `env_set`. El repo **no** declara `allow(unused_unsafe)` en ningún
nivel; bajo `{{BASELINE.toolchain.channel}}`/2021 eso compila por el
carve-out del lint para la transición a 2024. F0 debe **confirmarlo con un
snippet mínimo compilado con el toolchain real** y adjuntar la salida: es una
verificación, no una suposición.

---

## 6. Plan y gates (sin promesas que el repo no cumple)

| Fase | Contenido | Gate de salida |
|---|---|---|
| **F0** | RF-01, RF-02, re-auditoría, `baseline.json`, tag `migration/pre-1100` sobre `{{BASELINE.head_sha}}`, inventario regenerado, **todas las aprobaciones de §7** | `check_migration_evidence.sh` verde · `ci_fast_gate.sh` verde en clon con historia y tags · evidencia adjunta al issue padre |
| **F1** | RF-03..RF-12, RF-22, RF-23, RF-24, MIG-29 | drift guard + autotest · pins · clippy/check/test · compat 5/5 · **{{BASELINE.decisions.miri_gate_wording}}** · seeds re-publicados · Tier1 p90 |
| **F2** | RF-13..RF-15 (flip atómico), fixtures con causa, corte `{{BASELINE.decisions.major_target}}` | 5 miembros + 2 excluidos en 2024 · `corpus_determinism` byte-idéntico · Windows typecheck · sanitizers · advisory Miri revisado |
| **F3** | RF-16..RF-21 | K3/K4 = 0, K5 ≤20 · mutación y cobertura no decrecen · ratchet verde |

**Calendario.** F1 el **2026-11-26**: se respeta la ventana de 14 días que el
propio v1 recomendaba y, con el repo ya en `{{BASELINE.workspace_version}}`,
no hay urgencia. `1.101.0` cae dentro de F2/F3 y no se persigue (política
trimestral de RF-22).

**F0 exige un clon con historia y tags**, no un shallow: `check_support_drift.sh`
es fail-closed y trata el shallow clone como error, y los tags de recuperación
de §12 no existen en uno. Es un requisito del entorno, no del plan.

**Reversión.** F1: revert del commit atómico + seed
`{{BASELINE.toolchain.channel}}` **protegido** hasta publicar. F2: revert del
commit de edición (no «por crate»). F3: revert por lote. Release: la línea
`{{BASELINE.decisions.backport_line}}` sigue en
`{{BASELINE.decisions.msrv_compat_toolchain}}`/2021 y acepta seguridad.

---

## 7. Aprobaciones — **todas en F0**

| Decisión | Propuesta |
|---|---|
| Aprobar F0–F3 y destino `1.100.0` | sí, con F1 el 2026-11-26 |
| MSRV como ruptura | **`{{BASELINE.decisions.major_target}}`**, rationale reescrito (B3) |
| Línea de backports 1.91 | **`{{BASELINE.decisions.backport_line}}`**, o excepción de soporte explícita |
| Miri | **advisory**, no gate (B4); promoverlo queda fuera de este PRD |
| Ratchet `check_api_surface_ratchet.sh` | sí, fail-closed, wired + `# NEGATION:` en el mismo lote, aprobado aquí |
| Seam MCP fuera de proceso | sí, con las tres condiciones de contención |
| `docs/` | autorización **sólo para pins** + matiz de la frase de Miri en README |
| `TRAIN_OPEN` | sí mientras el tren esté abierto; implica re-aprobar §7 tras cada release que rote soporte |

---

## 8. Contrato de evidencia (cómo se opera)

```bash
# F0: generar/congelar la evidencia sobre el SHA de partida
python3 scripts/audit_migration_baseline.py --write evidence/mig-2026-01/baseline.json

# Cada PR: gate (también corre en repo-guards)
bash scripts/check_migration_evidence.sh
bash scripts/test_migration_evidence.sh        # harness de negación, #2056

# Cuando el repo se mueve (release, rotación de soporte, bump de canal):
python3 scripts/audit_migration_baseline.py --write evidence/mig-2026-01/baseline.json
git diff evidence/mig-2026-01/baseline.json   # revisar QUÉ decisión cambió
# re-aprobar §7 y commitear baseline.json + PRD-v2.md en el mismo lote
```

El gate comprueba: JSON bien formado y completo, `{{BASELINE.*}}` resueltos,
**cero hechos literales** del v1, **cero RFs huérfanos**, SHA del baseline
ancestro de HEAD, y decisiones que se re-derivan de `Cargo.toml`,
`rust-toolchain.toml`, `support.json` y `README.md`. Con `TRAIN_OPEN` presente
la deriva bloquea; sin él, degrada a `NOTE` para no ensuciar main cuando el
tren ya cerró. Los contadores son evidencia de F0 y **no** se comparan en CI:
un gate que se pone rojo cada vez que alguien agrega un test se desactiva, y
un gate desactivado es peor que ninguno.

---

## 9. Límites de este documento

- No se ejecutó el bump, ni el flip de edición, ni ningún cambio de CI fuera
  del gate de evidencia: este entregable es **F0**, no F1.
- No hay `cargo`/`rustc` en el sandbox de la re-auditoría; `unused_unsafe` y
  la compilación con `1.100.0` quedan como verificaciones de F0 con el
  toolchain real (RF-02, RF-05).
- Las notas upstream de 1.92→1.100 se toman como verificadas por la auditoría
  entrante (fuentes primarias citadas en el Anexo D del v1); este trabajo
  re-auditó **el repo**, que es donde estaba el error.
- `PRD-v1.md` se conserva sin editar como evidencia de lo que se corrigió.
