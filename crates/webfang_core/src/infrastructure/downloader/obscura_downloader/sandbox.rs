//! Cohesive helper for applying the Layer 3 sandbox launch decision to a
//! chromiumoxide browser configuration.

use crate::infrastructure::downloader::obscura_downloader::sandbox::{
    SandboxLaunch, REAL_BINARY_ENV,
};
use chromiumoxide::browser::BrowserConfigBuilder;

/// Fold the sandbox launch decision into `builder`.
///
/// The wrapper consumes and strips `REAL_BINARY_ENV` before Chrome is exec'd.
/// A missing prerequisite is never fatal: Layer 3 degrades to the configured
/// or auto-detected binary.
pub(crate) fn apply_launch(
    mut builder: BrowserConfigBuilder,
    launch: SandboxLaunch,
) -> BrowserConfigBuilder {
    if let Some(path) = &launch.executable {
        builder = builder.chrome_executable(path);
    }
    if let Some(binary) = &launch.real_binary {
        builder = builder.env(REAL_BINARY_ENV, binary.to_string_lossy().into_owned());
    }
    builder
}
