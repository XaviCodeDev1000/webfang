#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

//! Contract: the workspace panic lints (#2354) stay wired to every member.
//!
//! Pins the enforcement chain that makes a new `unwrap()`/`expect()` in any
//! production target — including the `webfang_mcp` bins, which the old
//! per-crate `cfg_attr(not(test), deny)` copies never covered — fail
//! `cargo clippy --all-targets`: the deny is declared once in
//! `[workspace.lints.clippy]`, and every workspace member inherits it through
//! `[lints] workspace = true` (Cargo applies package lints to all targets,
//! bins and benches included). The `cfg_attr(test, allow(...))` carve-out at
//! each root keeps tests exempt without weakening production enforcement.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("manifest dir sits directly under crates/")
        .parent()
        .expect("crates/ sits directly under the workspace root")
        .to_path_buf()
}

fn workspace_manifest() -> String {
    fs::read_to_string(workspace_root().join("Cargo.toml"))
        .expect("workspace Cargo.toml is readable")
}

/// Returns the body of `header` (e.g. `[workspace.lints.clippy]`), up to the
/// next line that starts a new table.
fn toml_section(manifest: &str, header: &str) -> Option<String> {
    let start = manifest.find(header)? + header.len();
    let rest = &manifest[start..];
    let end = rest
        .lines()
        .skip(1)
        .position(|line| line.trim_start().starts_with('['))
        .map(|line_idx| {
            rest.lines()
                .take(line_idx + 1)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_else(|| rest.to_string());
    Some(end)
}

#[test]
fn issue_2354_workspace_declares_panic_lint_denies() {
    let manifest = workspace_manifest();
    let section = toml_section(&manifest, "[workspace.lints.clippy]")
        .expect("[workspace.lints.clippy] exists in the root Cargo.toml");
    for deny in [
        "unwrap_used = { level = \"deny\" }",
        "expect_used = { level = \"deny\" }",
    ] {
        assert!(
            section.contains(deny),
            "[workspace.lints.clippy] must declare `{deny}` — the centralized \
             panic-lint gate (#2354) was removed or weakened"
        );
    }
}

#[test]
fn issue_2354_every_member_inherits_workspace_lints() {
    let manifest = workspace_manifest();
    let members_section = toml_section(&manifest, "members = [")
        .expect("[workspace] members list exists in the root Cargo.toml");
    let members: Vec<&str> = members_section
        .lines()
        .filter_map(|line| {
            let entry = line.trim().trim_end_matches(',');
            (!entry.is_empty() && entry.starts_with('"')).then(|| entry.trim_matches('"'))
        })
        .collect();
    assert!(
        members.len() >= 5,
        "expected the five workspace members, found: {members:?}"
    );
    for member in members {
        let member_manifest = fs::read_to_string(workspace_root().join(member).join("Cargo.toml"))
            .unwrap_or_else(|err| panic!("member {member} manifest is readable: {err}"));
        assert!(
            member_manifest.contains("[lints]\nworkspace = true"),
            "member {member} must inherit the workspace lints via \
             `[lints] workspace = true` — without it a new crate is born \
             without the panic-lint gate (#2354)"
        );
    }
}

#[test]
fn issue_2354_mcp_bins_carry_the_test_carve_out() {
    for bin in ["mcp_server_http.rs", "mcp_server_stdio.rs"] {
        let bin_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/bin")
            .join(bin);
        let source = fs::read_to_string(&bin_path)
            .unwrap_or_else(|err| panic!("bin {bin} is readable: {err}"));
        assert!(
            source.contains("cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))"),
            "bin {bin} must carry the test-only carve-out so its cfg(test) \
             mods stay exempt from the workspace panic lints (#2354)"
        );
    }
}
