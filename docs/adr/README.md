# Architecture Decision Records — índice

Este directorio es el corpus de decisiones arquitectónicas del proyecto. Este índice es la
superficie **citable**: un número de ADR sin este mapa es ambiguo, y una decisión que no se
puede citar no se puede auditar.

## Convenciones

| Convención | Regla |
| :--- | :--- |
| Nombre de archivo | `NNNN-slug.md`, cuatro dígitos, slug en kebab-case |
| Estado | `**Status:**` en el encabezado del archivo — `Accepted`, `Proposed`, `Superseded by NNNN`, `Deprecated` |
| Cita en código | `ADR-NNNN` (o la clave de la tabla de abajo cuando el número está compartido) |
| Numeración | Nunca se reutiliza un número. Un hueco es historia, no un error |

## Índice

| Clave de cita | Archivo | Decisión | Estado |
| :--- | :--- | :--- | :--- |
| `ADR-0001` | [0001-unify-asset-downloader.md](0001-unify-asset-downloader.md) | Unificar la descarga de assets en el `Downloader` de adapters | Accepted (amend. 2026-09-04) |
| `ADR-0003` | [0003-remote-inference-adapter.md](0003-remote-inference-adapter.md) | Adapter de inferencia remoto OpenAI-compatible como vía AI por defecto | Accepted |
| `ADR-0004` | [0004-onnx-local-removal.md](0004-onnx-local-removal.md) | Retirar ONNX local; embeddings remotos como única vía | Accepted — shipped |
| `ADR-0009` | [0009-persistence-mode-input-struct.md](0009-persistence-mode-input-struct.md) | `PersistenceMode` input struct: domain dueña la regla, application el wiring | Accepted |
| `ADR-0010` | [0010-intra-crate-direction-allowlist.md](0010-intra-crate-direction-allowlist.md) | Gate estricto de dirección intra-crate: ports híbridos + allowlist | Accepted |
| `ADR-0011` | [0011-tighten-intra-crate-allowlist.md](0011-tighten-intra-crate-allowlist.md) | Endurecer la allowlist: quitar `infrastructure::` amplio | Accepted |
| `ADR-0012` | [0012-intra-crate-allowlist-roadmap.md](0012-intra-crate-allowlist-roadmap.md) | Roadmap para vaciar la allowlist intra-crate (#994) | Accepted |
| `ADR-0012-B` | [0012-intra-crate-allowlist-roadmap-b.md](0012-intra-crate-allowlist-roadmap-b.md) | Plan corregido 10→2 de la allowlist | **Proposed** — *stale*: el plan figura completo desde 2026-09-03 y la allowlist está en 2 entradas |
| `ADR-0012-B-RUNBOOK` | [0012-B-RUNBOOK.md](0012-B-RUNBOOK.md) | Runbook del estado operativo medido de 0012-B (no es una decisión) | n/a (runbook) |
| `ADR-0013` | [0013-state-store-port-seam.md](0013-state-store-port-seam.md) | `StateStorePort`: seam de dominio sobre el state store legacy | Accepted |
| `ADR-0014` | [0014-persistence-unification-hybrid-d3.md](0014-persistence-unification-hybrid-d3.md) | Unificación de persistencia y estrategia de commit-point híbrida D3 | Accepted |
| `ADR-0015` | [0015-waf-detection-stays-infrastructure-side.md](0015-waf-detection-stays-infrastructure-side.md) | WAF detection se queda del lado de infraestructura (#994 slice 4) | Accepted |
| `ADR-0016` | [0016-interruption-resume-lifecycle.md](0016-interruption-resume-lifecycle.md) | Ciclo interrupción/resume: dueño único, precedencia fijada, residual F-R3-7 | Accepted |
| `ADR-0016-POST-LOAD-WAIT` | [0016-post-load-wait-policy.md](0016-post-load-wait-policy.md) | Política de espera post-load: network-idle por defecto (F-52-b) | Accepted — shipped |
| `ADR-0017` | [0017-crawlsession-run-owner.md](0017-crawlsession-run-owner.md) | `CrawlSession`: un dueño único por corrida | Accepted |
| `ADR-0018` | [0018-egress-strategy-without-third-party-proxies.md](0018-egress-strategy-without-third-party-proxies.md) | Estrategia de egress sin proxies de terceros | Proposed — draft |
| `ADR-0019` | [0019-boundedness-lives-in-the-capture-sink.md](0019-boundedness-lives-in-the-capture-sink.md) | La acotación vive en el capture sink; el bloqueo está fuera de alcance | Accepted |

**Total: 17 archivos, 16 decisiones + 1 runbook.**

## Huecos de numeración

`0002`, `0005`, `0006`, `0007` y `0008` **nunca existieron**. Verificado con
`git log --diff-filter=A -- "docs/adr/*"`: ningún archivo con esos números se agregó jamás.
No hay pérdida de historial que buscar.

## Por qué hay números compartidos

`ADR-0012` tiene tres archivos y `ADR-0016` dos. No es un error de tipeo: son decisiones
escritas en la misma ola, y el runbook heredó el número del plan que documenta.

**Estos archivos NO se renumeran**, y la razón es medible: `ADR-0012` aparece **143 veces**
en código, scripts y documentación; renombrar el archivo sin migrar las citas rompe más de
lo que arregla. La ambigüedad se resuelve **en este índice**, que es la superficie que un
lector consulta:

- `ADR-0012` → el roadmap (accepted), el plan original.
- `ADR-0012-B` → el plan corregido (10→2), el que se ejecutó.
- `ADR-0012-B-RUNBOOK` → el estado operativo medido, no una decisión.

Lo mismo con 0016:

- `ADR-0016` → ciclo interrupción/resume. **Es el que se cita con `§4`, `§5` y `§6`**
  (P8-4/P8-5/P8-6, residual F-R3-7).
- `ADR-0016-POST-LOAD-WAIT` → política de espera post-load (F-52-b). No tiene secciones
  numeradas citadas desde código.

Una renumeración real (con migración de las 143 citas) es deuda abierta, no un cambio de
este índice.

## Reglas para un ADR nuevo

1. Tomá el siguiente número libre **después del máximo**, nunca un hueco.
2. Escribí `**Status:**` en el encabezado, con la fecha si el estado cambió.
3. Agregá la fila a la tabla de este índice **en el mismo commit**. Un ADR sin fila no se
   puede citar.
4. Si la decisión reemplaza a otra, poné `Superseded by NNNN` en el viejo y `Supersedes`
   en el nuevo.
