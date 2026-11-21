use super::*;
use std::path::PathBuf;

fn default_dir() -> PathBuf {
    PathBuf::from("/tmp/default_state")
}

fn cfg(resume: bool, state_dir: Option<&str>, interval: u64, no_checkpoint: bool) -> ResumeConfig {
    ResumeConfig {
        resume,
        state_dir: state_dir.map(PathBuf::from),
        checkpoint_interval: interval,
        no_checkpoint,
    }
}

// ——— 8-combo matrix ———

#[test]
fn disabled_when_no_resume_and_checkpoint_disabled_by_zero() {
    let m = PersistenceMode::from_config(&cfg(false, None, 0, false), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
}

#[test]
fn disabled_when_no_resume_and_no_checkpoint_flag() {
    let m = PersistenceMode::from_config(&cfg(false, None, 100, true), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
}

#[test]
fn resume_only_with_default_dir() {
    let m = PersistenceMode::from_config(&cfg(true, None, 0, false), &default_dir());
    assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
}

#[test]
fn resume_with_custom_state_dir() {
    let m = PersistenceMode::from_config(&cfg(true, Some("/tmp/cache"), 0, false), &default_dir());
    assert_eq!(
        m,
        PersistenceMode::Resume {
            dir: PathBuf::from("/tmp/cache")
        }
    );
}

#[test]
fn default_resume_config_resolves_to_disabled() {
    // F-01: a bare `ResumeConfig::default()` (no --resume, no --state-dir,
    // interval 0) must never enable persistence. Repeat crawls stay
    // deterministic unless the operator opts in via --resume/--state-dir.
    let m = PersistenceMode::from_config(&ResumeConfig::default(), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
}

#[test]
fn repeat_crawl_without_opt_in_is_disabled() {
    // F-01 INVERT (Option A): (false, None, N>0, false) → Disabled.
    // A repeat crawl with no --resume and no --state-dir must not pick up
    // checkpoint state from the shared default dir — otherwise the second
    // identical crawl resumes from the first and produces a different
    // output set. Checkpoint requires an explicit opt-in (--resume or
    // --state-dir); the default interval alone never enables it.
    let m = PersistenceMode::from_config(&cfg(false, None, 100, false), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
    assert!(m.checkpoint_cfg().is_none());
}

#[test]
fn checkpoint_stays_disabled_without_opt_in_custom_interval() {
    // Same invert as above with a non-default interval: without --resume
    // or --state-dir there is no opt-in, so any N>0 still resolves to
    // Disabled.
    let m = PersistenceMode::from_config(&cfg(false, None, 50, false), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
    assert!(m.checkpoint_cfg().is_none());
}

#[test]
fn full_with_resume_and_checkpoint() {
    let m = PersistenceMode::from_config(&cfg(true, Some("/tmp/x"), 50, false), &default_dir());
    assert_eq!(
        m,
        PersistenceMode::Full {
            resume_dir: PathBuf::from("/tmp/x"),
            checkpoint: CheckpointCfg {
                dir: PathBuf::from("/tmp/x"),
                interval: 50
            }
        }
    );
}

#[test]
fn full_with_default_dir_and_interval_100() {
    let m = PersistenceMode::from_config(&cfg(true, None, 100, false), &default_dir());
    assert_eq!(
        m,
        PersistenceMode::Full {
            resume_dir: default_dir(),
            checkpoint: CheckpointCfg {
                dir: default_dir(),
                interval: 100
            }
        }
    );
}

// ——— Disable rules ———

#[test]
fn interval_zero_disables_checkpoint_even_with_resume() {
    let m = PersistenceMode::from_config(&cfg(true, None, 0, false), &default_dir());
    // With interval 0, checkpoint must be disabled → Resume only, not Full.
    assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
    assert!(m.checkpoint_cfg().is_none());
}

#[test]
fn no_checkpoint_disables_checkpoint() {
    let m = PersistenceMode::from_config(&cfg(true, None, 100, true), &default_dir());
    // no_checkpoint true → Resume only, interval ignored.
    assert_eq!(m, PersistenceMode::Resume { dir: default_dir() });
    assert!(m.checkpoint_cfg().is_none());
}

#[test]
fn checkpoint_disabled_when_interval_zero_even_without_resume() {
    let m = PersistenceMode::from_config(&cfg(false, None, 0, false), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
    assert!(m.checkpoint_cfg().is_none());
}

// ——— state_dir without --resume is ignored ———

#[test]
fn explicit_state_dir_without_resume_opts_into_checkpoint() {
    // Requirement (c): an explicit --state-dir is the checkpoint opt-in —
    // it is honoured (not ignored) and scopes the checkpoint to that dir.
    let m =
        PersistenceMode::from_config(&cfg(false, Some("/tmp/cache"), 100, false), &default_dir());
    assert_eq!(
        m,
        PersistenceMode::Checkpoint {
            cfg: CheckpointCfg {
                dir: PathBuf::from("/tmp/cache"),
                interval: 100
            }
        }
    );
}

#[test]
fn state_dir_without_resume_and_checkpoint_disabled_is_disabled() {
    let m = PersistenceMode::from_config(&cfg(false, Some("/tmp/cache"), 0, false), &default_dir());
    assert_eq!(m, PersistenceMode::Disabled);
}

// ——— ResolverNotes side-channel (#1045) ———

#[test]
fn notes_report_ignored_state_dir_only_when_disabled() {
    // ResolverNotes.ignored_state_dir is Some exactly when the resolved
    // mode is Disabled and a --state-dir was passed (i.e. the flag had no
    // effect). A checkpoint-enabled run consumes --state-dir, so no note.
    let (_, notes) = PersistenceMode::from_config_with_notes(
        &cfg(false, Some("/tmp/cache"), 0, false),
        &default_dir(),
    );
    assert_eq!(notes.ignored_state_dir, Some(PathBuf::from("/tmp/cache")));
}

#[test]
fn notes_empty_when_state_dir_opts_into_checkpoint() {
    // (false, Some(dir), N>0, false) → Checkpoint{dir}: the flag is
    // honoured, so there is nothing to report.
    let (_, notes) = PersistenceMode::from_config_with_notes(
        &cfg(false, Some("/tmp/cache"), 100, false),
        &default_dir(),
    );
    assert_eq!(notes, ResolverNotes::default());
}

#[test]
fn notes_empty_when_resume_consumes_state_dir() {
    let (_, notes) = PersistenceMode::from_config_with_notes(
        &cfg(true, Some("/tmp/cache"), 0, false),
        &default_dir(),
    );
    assert_eq!(notes, ResolverNotes::default());
}

#[test]
fn notes_empty_when_no_state_dir_given() {
    // No --state-dir: nothing to ignore. Uses an opt-out case (Disabled
    // via --no-checkpoint) so the assertion holds under the F-01 truth
    // table regardless of the interval default.
    let (_, notes) =
        PersistenceMode::from_config_with_notes(&cfg(false, None, 100, true), &default_dir());
    assert_eq!(notes, ResolverNotes::default());
}

#[test]
fn from_config_agrees_with_notes_variant_on_full_matrix() {
    // `from_config` is the notes-dropping façade: same mode, no notes.
    for (resume, state_dir, interval, no_checkpoint) in [
        (false, None, 0, false),
        (false, None, 100, false),
        (false, Some("/tmp/cache"), 100, false),
        (false, Some("/tmp/cache"), 0, false),
        (true, Some("/tmp/cache"), 50, false),
        (true, None, 100, true),
    ] {
        let c = cfg(resume, state_dir, interval, no_checkpoint);
        let (mode, _) = PersistenceMode::from_config_with_notes(&c, &default_dir());
        assert_eq!(PersistenceMode::from_config(&c, &default_dir()), mode);
    }
}

// ——— helpers ———

#[test]
fn is_resume_helpers() {
    assert!(!PersistenceMode::Disabled.is_resume());
    assert!(PersistenceMode::Resume { dir: default_dir() }.is_resume());
    assert!(!PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: default_dir(),
            interval: 100
        }
    }
    .is_resume());
    assert!(PersistenceMode::Full {
        resume_dir: default_dir(),
        checkpoint: CheckpointCfg {
            dir: default_dir(),
            interval: 100
        }
    }
    .is_resume());
}

#[test]
fn checkpoint_cfg_helper() {
    assert!(PersistenceMode::Disabled.checkpoint_cfg().is_none());
    assert!(PersistenceMode::Resume { dir: default_dir() }
        .checkpoint_cfg()
        .is_none());
    let cp = PersistenceMode::Checkpoint {
        cfg: CheckpointCfg {
            dir: default_dir(),
            interval: 42,
        },
    };
    assert_eq!(cp.checkpoint_cfg().unwrap().interval, 42);
    let full = PersistenceMode::Full {
        resume_dir: default_dir(),
        checkpoint: CheckpointCfg {
            dir: default_dir(),
            interval: 99,
        },
    };
    assert_eq!(full.checkpoint_cfg().unwrap().interval, 99);
}
