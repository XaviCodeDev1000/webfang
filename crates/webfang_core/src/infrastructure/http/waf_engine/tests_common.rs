//! Base compartida de los tests de `waf_engine`.
//!
//! Extraido del `mod tests` inline de `waf_engine.rs` (PR 1 de la mision Ola 1,
//! paraguas #2584). `ctx_with` no vive con los tests de verdict a pesar de haber
//! nacido ahi: sus consumidores son tres hijos distintos (`tests_evidence` por el
//! cap de evidencia, `tests_inspect` por la politica de verdict y `tests_entropy`
//! por las filas de entropia), asi que el destino lo dictan los consumidores y no
//! la comodidad. `pub(super)` = visible en el modulo `waf_engine` y sus descendientes.

use crate::domain::waf::InspectionContext;

pub(super) fn ctx_with(status: Option<u16>, content_type: Option<&str>) -> InspectionContext {
    InspectionContext {
        status,
        content_type: content_type.map(String::from),
        ..Default::default()
    }
}
