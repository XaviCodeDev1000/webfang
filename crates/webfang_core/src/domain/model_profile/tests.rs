use super::*;

fn granite_97m() -> ModelProfile {
    builtin_profiles()
        .into_iter()
        .find(|p| p.id == GRANITE_97M_PROFILE_ID)
        .expect("builtin profile")
}

#[test]
fn builtin_profiles_are_valid_and_explicitly_versioned() {
    for profile in builtin_profiles() {
        assert_eq!(profile.schema_version, PROFILE_SCHEMA_VERSION);
        profile
            .validate()
            .unwrap_or_else(|e| panic!("shipped profile must be valid: {e}"));
        assert!(profile.verified);
        assert_eq!(profile.provenance, ProfileProvenance::RepoVerified);
        assert!(!profile.citation.is_empty(), "provenance must be cited");
    }
}

#[test]
fn override_config_parses_from_a_json_block_and_is_empty_by_default() {
    let parsed: ModelProfilesConfig =
        serde_json::from_str(r#"{"profiles": [{"id": "granite-97m", "batch_max": 8}]}"#)
            .expect("override block parses");
    assert_eq!(parsed.len(), 1);
    assert!(parsed
        .find("granite-97m")
        .is_some_and(|o| o.batch_max == Some(8)));
    assert!(ModelProfilesConfig::default().is_empty());
}

#[test]
fn an_override_leaves_untouched_fields_alone() {
    let base = granite_97m();
    let patched = base.with_override(&ModelProfileOverride {
        id: base.id.clone(),
        chars_per_token: Some(2.0),
        ..ModelProfileOverride::default()
    });
    assert_eq!(patched.chars_per_token, 2.0);
    assert_eq!(patched.dim, base.dim);
    assert_eq!(patched.context, base.context);
    assert!(patched.verified);
}

#[test]
fn validate_rejects_an_unpinned_or_zeroed_profile() {
    let mut profile = granite_97m();
    profile.schema_version = PROFILE_SCHEMA_VERSION + 1;
    assert!(matches!(
        profile.validate(),
        Err(ProfileError::UnknownSchemaVersion { .. })
    ));

    let mut profile = granite_97m();
    profile.dim = 0;
    assert!(matches!(
        profile.validate(),
        Err(ProfileError::InvalidField { field: "dim", .. })
    ));
}

/// S10/T2b: the budget the run chunks with and the budget the diagnostics
/// print come from ONE resolution. Without `--max-chars` it is the effective
/// profile's `max_chars()`, operator overrides included.
#[test]
fn governed_budget_comes_from_the_effective_profile() {
    let profiles = ModelProfilesConfig {
        profiles: vec![ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            context: Some(1024),
            chars_per_token: Some(2.0),
            ..ModelProfileOverride::default()
        }],
    };
    assert_eq!(governed_max_chars(None, &profiles).expect("resolves"), 2048);
}

#[test]
fn governed_budget_lets_an_explicit_flag_win() {
    let profiles = ModelProfilesConfig {
        profiles: vec![ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            context: Some(1024),
            chars_per_token: Some(2.0),
            ..ModelProfileOverride::default()
        }],
    };
    assert_eq!(
        governed_max_chars(Some(500), &profiles).expect("resolves"),
        500
    );
}

#[test]
fn governed_budget_without_overrides_matches_the_spec_default() {
    assert_eq!(
        governed_max_chars(None, &ModelProfilesConfig::default()).expect("resolves"),
        crate::domain::options_spec::ai::DEFAULT_MAX_CHARS
    );
}

#[test]
fn governed_budget_refuses_an_invalid_override() {
    let profiles = ModelProfilesConfig {
        profiles: vec![ModelProfileOverride {
            id: GRANITE_97M_PROFILE_ID.to_string(),
            context: Some(0),
            ..ModelProfileOverride::default()
        }],
    };
    assert!(
        governed_max_chars(None, &profiles).is_err(),
        "a zero context must fail closed instead of silently chunking at zero"
    );
}
