//! Shared fixtures for the normalization pipeline test modules (M3 PR B split).
use crate::cli::config::ConfigDefaults;
use crate::Args;

pub(super) fn dummy_args() -> Args {
    Args::default()
}

pub(super) fn dummy_config() -> ConfigDefaults {
    ConfigDefaults::default()
}
