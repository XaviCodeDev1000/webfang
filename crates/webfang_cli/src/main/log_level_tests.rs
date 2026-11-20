use super::resolve_log_level_with_config;

#[test]
fn explicit_flags_win_over_config() {
    assert_eq!(resolve_log_level_with_config(1, Some("trace")), "info");
    assert_eq!(resolve_log_level_with_config(2, Some("error")), "debug");
    assert_eq!(resolve_log_level_with_config(3, Some("info")), "trace");
}

#[test]
fn config_level_applies_when_no_flags() {
    assert_eq!(resolve_log_level_with_config(0, Some("info")), "info");
    assert_eq!(resolve_log_level_with_config(0, Some("TRACE")), "trace");
    assert_eq!(resolve_log_level_with_config(0, Some("  debug  ")), "debug");
    assert_eq!(resolve_log_level_with_config(0, Some("warn")), "warn");
}

#[test]
fn missing_or_invalid_config_falls_back_to_warn() {
    assert_eq!(resolve_log_level_with_config(0, None), "warn");
    assert_eq!(resolve_log_level_with_config(0, Some("")), "warn");
    assert_eq!(resolve_log_level_with_config(0, Some("   ")), "warn");
    assert_eq!(resolve_log_level_with_config(0, Some("loud")), "warn");
}
