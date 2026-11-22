//! A `#[cfg(test)]` mod declared with its content in another file makes the
//! declared child file a test region.
//!
//! The file-size program extracts inline `#[cfg(test)]` mods into `*_tests.rs`
//! siblings wired from the parent (`#[cfg(test)] #[path = "..."] mod tests;`).
//! The gate lives at the declaration site, so the child file carries no `cfg`
//! attribute of its own — without the declaration pass the extractor would
//! scan it as production code and report the intra-crate references that the
//! same references never produced while they lived inline in the parent.

#[path = "common/mod.rs"]
mod common;

use common::TempTree;

const PARENT: &str = r#"//! The production module.
use crate::domain::Production;

#[cfg(test)]
#[path = "wired_tests.rs"]
mod tests;

// The B36 property survives: production code after the declaration is still
// scanned.
use crate::infrastructure::AfterDeclaration;
"#;

const WIRED_TEST_FILE: &str = r#"//! The extracted test file: its gate is the parent's declaration.
use super::*;
use crate::infrastructure::InsideWiredTestFile;
"#;

const UNGATED_PARENT: &str = r#"//! A declaration without a `cfg(test)` predicate is NOT a test region.
use crate::domain::Production;

#[path = "unwired_tests.rs"]
mod tests;
"#;

const UNWIRED_TEST_FILE: &str = r#"use crate::infrastructure::InsideUnwiredFile;
"#;

const BARE_PARENT: &str = r#"//! Default resolution: `#[cfg(test)] mod tests;` loads the `tests.rs` sibling.
use crate::domain::Production;

#[cfg(test)]
mod tests;
"#;

const BARE_TEST_FILE: &str = r#"use crate::infrastructure::InsideBareTestFile;
"#;

#[test]
fn wired_test_file_is_excluded_whole() {
    let tree = TempTree::new("declared-wired");
    tree.write("application/parent.rs", PARENT);
    tree.write("application/wired_tests.rs", WIRED_TEST_FILE);
    let records = tree.records();
    assert!(
        !records.iter().any(|r| r.contains("InsideWiredTestFile")),
        "the wired test file must be a test region by declaration: {records:?}"
    );
    assert!(
        records.iter().any(|r| r.contains("AfterDeclaration")),
        "production code after the declaration is still scanned: {records:?}"
    );
}

#[test]
fn ungated_declaration_is_not_a_test_region() {
    let tree = TempTree::new("declared-ungated");
    tree.write("application/parent.rs", UNGATED_PARENT);
    tree.write("application/unwired_tests.rs", UNWIRED_TEST_FILE);
    let records = tree.records();
    assert!(
        records.iter().any(|r| r.contains("InsideUnwiredFile")),
        "without the cfg(test) predicate the file is production code: {records:?}"
    );
}

#[test]
fn bare_declaration_resolves_the_default_sibling() {
    let tree = TempTree::new("declared-bare");
    tree.write("application/parent.rs", BARE_PARENT);
    tree.write("application/tests.rs", BARE_TEST_FILE);
    let records = tree.records();
    assert!(
        !records.iter().any(|r| r.contains("InsideBareTestFile")),
        "a bare `#[cfg(test)] mod tests;` makes tests.rs a test region: {records:?}"
    );
}
