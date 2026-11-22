//! Resolution of the `webfang` binary path for integration tests: explicit
//! inputs, the workspace-root climb, the runner pre-build contract, and the
//! build-on-demand fallback.

use std::env;
use std::path::{Path, PathBuf};

/// Resolve the workspace root by climbing from a crate manifest directory.
///
/// Every workspace member lives at `crates/<name>`, so ONE `parent()` hop
/// reaches `crates/` and TWO reach the root that holds the virtual
/// manifest. The #1366 bug climbed three hops — one too many — landing
/// in the repo's PARENT, so the no-env fallback pointed at a `target/`
/// directory cargo never populates.
///
/// # Panics
///
/// Panics if `manifest_dir` is not at least two levels below a root —
/// impossible for a workspace member compiled in-tree.
pub(crate) fn workspace_root_from_manifest(manifest_dir: &str) -> PathBuf {
    PathBuf::from(manifest_dir)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .expect("resolve workspace root from CARGO_MANIFEST_DIR")
}

/// Pure core of [`webfang_path`]: resolve the `webfang` build-output path
/// from EXPLICIT inputs — no environment reads, no process mutation.
///
/// `bin_exe` mirrors `CARGO_BIN_EXE_webfang` (set = trusted outright),
/// `target_dir` mirrors `CARGO_TARGET_DIR` (set = output root), and
/// `manifest_dir` anchors the fallback workspace-root climb. Passing
/// `None` for both variables simulates the #1366 environment — a runner
/// with neither harness variable set — hermetically, without touching
/// the process environment.
pub(crate) fn resolve_webfang_path(
    bin_exe: Option<&str>,
    target_dir: Option<&str>,
    manifest_dir: &str,
) -> PathBuf {
    if let Some(p) = bin_exe {
        return PathBuf::from(p);
    }
    let workspace_root = workspace_root_from_manifest(manifest_dir);
    let target_root = match target_dir {
        Some(dir) => PathBuf::from(dir),
        None => workspace_root.join("target"),
    };
    let mut built = target_root.join("debug").join("webfang");
    if cfg!(windows) {
        built.set_extension("exe");
    }
    built
}

/// Environment contract through which a **test runner** declares that it
/// already built the `webfang` binary for this run.
///
/// Set it to `"1"` in the runner's environment (job-level in CI), next to the
/// pre-build step that produced `<target>/debug/webfang`. The resolver then
/// trusts that binary instead of spawning `cargo build` from every test
/// process.
///
/// This exists because a test process is not a build orchestrator (#2331): in
/// `test-crossplatform`, eight test processes serialised on cargo's build lock
/// and the on-demand build inside `the_retired_shim_env_vars_are_ignored` cost
/// 83 s of a passing attempt, while the same build inside
/// `crawl_js_strategy_respects_timeout_secs` was charged to its 10 s
/// wall-clock contract. The runner builds once, before any test starts.
///
/// Local runs leave it unset, so the resolver keeps building on demand and
/// never reuses a binary the current feature set did not produce. The
/// pre-build in the lane MUST use the feature set the tests expect: the
/// default-feature lanes build `webfang_cli`'s defaults, and the
/// `--all-features` lanes build the five features that
/// `crates/webfang_core/tests/common/cli_harness.rs` forwards — the one
/// configuration every CLI behavioral test in those lanes runs (#2337).
pub const PREBUILT_BIN_ENV: &str = "WEBFANG_TEST_PREBUILT_BIN";

/// Whether `PREBUILT_BIN_ENV`'s value is the declared marker.
///
/// Pure so the contract is pinned without mutating the process environment.
pub(crate) fn prebuilt_bin_declared(value: Option<&str>) -> bool {
    value == Some("1")
}

/// Resolve the path to the `webfang` binary, building it on demand.
///
/// `webfang` is built by the `webfang_cli` crate (a workspace sibling),
/// so `CARGO_BIN_EXE_webfang` is only set for the crate that owns the binary.
/// This function falls back to building it via `cargo build`, **unless** the
/// runner declared a pre-built binary through [`PREBUILT_BIN_ENV`].
///
/// # Panics
///
/// Panics if the workspace root cannot be resolved from the crate manifest
/// directory, if `cargo` cannot be spawned, or if the build fails. When the
/// runner declared a pre-built binary, panics if that path does not exist.
#[must_use]
pub fn webfang_path() -> PathBuf {
    if let Ok(p) = env::var("CARGO_BIN_EXE_webfang") {
        return PathBuf::from(p);
    }
    let workspace_root = workspace_root_from_manifest(env!("CARGO_MANIFEST_DIR"));
    let target_dir = env::var("CARGO_TARGET_DIR").ok();
    let built = resolve_webfang_path(None, target_dir.as_deref(), env!("CARGO_MANIFEST_DIR"));
    if prebuilt_bin_declared(env::var(PREBUILT_BIN_ENV).ok().as_deref()) {
        assert!(
            built.is_file(),
            "{PREBUILT_BIN_ENV}=1 declares a pre-built binary, but {} does not exist",
            built.display()
        );
        return built;
    }
    // #1697/#2331: resolve ONCE per process. This function shells out to
    // `cargo build`; without the cache every call in a test process re-invoked
    // the compiler. The first call in a fresh target dir still pays the build
    // (that is the local contract); a declared runner pre-build skips it.
    static BUILT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    BUILT
        .get_or_init(|| {
            let cargo = option_env!("CARGO").unwrap_or("cargo");
            let status = std::process::Command::new(cargo)
                .args(["build", "-p", "webfang_cli", "--bin", "webfang", "--quiet"])
                .current_dir(&workspace_root)
                .status()
                .expect("spawn cargo to build webfang");
            assert!(status.success(), "cargo build --bin webfang failed");
            built
        })
        .clone()
}
