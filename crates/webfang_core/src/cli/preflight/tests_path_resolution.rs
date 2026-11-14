use super::path_resolution::*;

// ---- XP-S-04 (#1608) — platform-independent PATH resolution ----------

/// `PATHEXT` parsing: order preserved, empty entries dropped, casing
/// normalized, dotless entries get the leading dot.
#[test]
fn parse_pathext_preserves_order_and_normalizes() {
    assert_eq!(
        parse_pathext(".COM;.EXE;.BAT;.CMD"),
        [".com", ".exe", ".bat", ".cmd"]
    );
    assert_eq!(parse_pathext(".EXE;;  .BAT "), [".exe", ".bat"]);
    assert_eq!(parse_pathext("EXE;.BAT"), [".exe", ".bat"]);
    assert!(parse_pathext("").is_empty());
    assert!(parse_pathext(";;;").is_empty());
}

/// A bare name on a non-PATHEXT platform resolves as exactly one
/// candidate (the name itself).
#[test]
fn bare_name_candidates_without_pathext_is_the_name_alone() {
    assert_eq!(bare_name_candidates("chrome", None), ["chrome"]);
}

/// A bare name with `PATHEXT` expands to the exact name first, then the
/// PATHEXT-ordered extensions — the resolution order cmd.exe uses.
#[test]
fn bare_name_candidates_expands_pathext_in_order() {
    let candidates = bare_name_candidates("chrome", Some(".COM;.EXE;.BAT;.CMD"));
    assert_eq!(
        candidates,
        [
            "chrome",
            "chrome.com",
            "chrome.exe",
            "chrome.bat",
            "chrome.cmd"
        ]
    );
}

/// `first_existing_in_dirs` is dir-major: every candidate name is tried
/// inside a directory before moving to the next directory.
///
/// The `PATH` value is built with `std::env::join_paths`, never with a
/// hardcoded separator: a `:`-joined string is one bogus entry for
/// `split_paths` on Windows (which splits on `;` and keeps `C:` drive
/// letters intact), which is why this test failed there with "a
/// candidate must resolve" while passing on Linux/macOS.
#[test]
fn first_existing_in_dirs_prefers_earlier_directory() {
    let dir1 = tempfile::TempDir::new().expect("tempdir");
    let dir2 = tempfile::TempDir::new().expect("tempdir");
    // dir1 only has the SECOND candidate name; dir2 has the FIRST.
    std::fs::write(dir1.path().join("chromium"), "#!/bin/sh\n").expect("write");
    std::fs::write(dir2.path().join("google-chrome"), "#!/bin/sh\n").expect("write");

    let names = vec!["google-chrome".to_string(), "chromium".to_string()];
    let path_value = std::env::join_paths([dir1.path(), dir2.path()]).expect("joinable temp dirs");
    let resolved = first_existing_in_dirs(std::env::split_paths(&path_value), &names)
        .expect("a candidate must resolve");
    assert_eq!(resolved, dir1.path().join("chromium"));
}

/// Windows-shaped resolution through the injectable seam: the
/// `PATHEXT` expansion is supplied as DATA (`bare_name_candidates`),
/// never read from process env, so the Windows answer is exercised on
/// every platform. Dir-major order must hold for the expanded
/// candidates too: the earlier directory's `chromium.exe` (a PATHEXT
/// candidate) wins over the later directory's bare `chromium`.
#[test]
fn first_existing_in_dirs_resolves_pathext_expansion_in_earlier_directory() {
    let dir1 = tempfile::TempDir::new().expect("tempdir");
    let dir2 = tempfile::TempDir::new().expect("tempdir");
    // dir1 only has an extension-bearing candidate; dir2 has the bare name.
    std::fs::write(dir1.path().join("chromium.exe"), "MZ fake binary\n").expect("write");
    std::fs::write(dir2.path().join("chromium"), "#!/bin/sh\n").expect("write");

    let names = bare_name_candidates("chromium", Some(".COM;.EXE"));
    assert_eq!(names, ["chromium", "chromium.com", "chromium.exe"]);
    let path_value = std::env::join_paths([dir1.path(), dir2.path()]).expect("joinable temp dirs");
    let resolved = first_existing_in_dirs(std::env::split_paths(&path_value), &names)
        .expect("a PATHEXT candidate must resolve");
    assert_eq!(
        resolved,
        dir1.path().join("chromium.exe"),
        "the earlier directory's extension candidate must win dir-major"
    );
}

/// Directories named like a candidate are skipped (`is_file`, the
/// F-52-c / #1278 guard), and missing names resolve to `None`.
#[test]
fn first_existing_in_dirs_skips_directories_and_reports_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::create_dir(dir.path().join("chrome.exe")).expect("mkdir decoy");
    let names = vec!["chrome.exe".to_string()];
    assert!(
        first_existing_in_dirs(std::env::split_paths(dir.path().as_os_str()), &names).is_none()
    );
}
