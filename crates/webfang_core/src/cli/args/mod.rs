/// AI-powered semantic cleaning arguments.
pub mod ai;
/// Crawler and discovery configuration arguments.
pub mod crawler;
/// Export format and output configuration arguments.
pub mod export;
/// LLM provider flags: `--extract-with-llm` + provider selection.
pub mod llm;
/// Obsidian vault integration arguments.
pub mod obsidian;

pub use ai::AiArgs;
pub use crawler::CrawlerArgs;
pub use export::ExportArgs;
pub use llm::LlmArgs;
pub use obsidian::ObsidianArgs;

/// Whether this value came from an operator decision rather than the spec's
/// default, for any arg id.
///
/// THE provenance predicate for the arg modules: every "did somebody choose
/// this?" question — `--max-chars`, `--threshold`, and `--timeout-secs`
/// (#2490) — is answered here, so the meaning of "chosen" cannot drift
/// between them. `clap::ArgMatches::value_source` is the only place that
/// distinction exists.
pub(crate) fn chosen_by_operator(m: &clap::ArgMatches, id: &str) -> bool {
    matches!(
        m.value_source(id),
        Some(clap::parser::ValueSource::CommandLine) | Some(clap::parser::ValueSource::EnvVariable)
    )
}

use crate::domain::ValidUrl;
use clap::Parser;

/// Test-only helpers shared by the per-group arg modules.
#[cfg(test)]
pub(crate) mod test_support {
    use crate::domain::options_spec::OptionSpec;
    use clap::Parser as _;

    /// Collect every clap arg from an already-augmented command.
    ///
    /// Call sites pass their group's augmented command
    /// (`LlmArgs::augment_args(Command::new(...))`); the returned args are
    /// then fed to the `assert_*` builders below with the group's spec.
    pub(crate) fn collect_args(cmd: clap::Command) -> Vec<clap::Arg> {
        cmd.get_arguments().cloned().collect()
    }

    /// Find one arg by id, panicking with the owning group's name so
    /// failures point at the right harness.
    pub(crate) fn arg_by_id<'a>(
        args: &'a [clap::Arg],
        id: &str,
        owner: &'static str,
    ) -> &'a clap::Arg {
        args.iter()
            .find(|a| a.get_id() == id)
            .unwrap_or_else(|| panic!("arg `{id}` missing from {owner} command"))
    }

    /// Parse a full `Args` from synthetic argv, with the ambient clap env
    /// fallbacks cleared.
    ///
    /// This is the ONLY parse entry point of the crate's test surface, and
    /// every test parse is therefore ALWAYS hermetic: a test can never
    /// observe the ambient shell (issue #2234), and never observes the
    /// poisoned window another test opens (issue #2341). The hermeticity is
    /// structural, not a convention — there is no non-hermetic sibling left
    /// to call by mistake.
    pub(crate) fn parse_args_hermetic(extra: &[&str]) -> Result<crate::Args, String> {
        let _window = clap_env_window();
        parse_args_hermetic_in_window(extra)
    }

    /// [`parse_args_hermetic`] for a caller that already holds [`clap_env_window`].
    ///
    /// Only the ambient-environment test needs this shape: it must span
    /// `set → parse → restore` inside one window, so the parse it performs
    /// cannot re-acquire the non-reentrant window lock.
    pub(crate) fn parse_args_hermetic_in_window(extra: &[&str]) -> Result<crate::Args, String> {
        with_clap_env_cleared(|| {
            let mut argv = vec!["webfang"];
            argv.extend_from_slice(extra);
            crate::Args::try_parse_from(argv).map_err(|e| e.to_string())
        })
    }

    /// The clap-env window: a lock that spans a whole *set → parse → restore*
    /// sequence, so no sibling parse can slip inside it (issue #2341).
    ///
    /// `ENV_LOCK` alone cannot express that window: `env_set` and the parse's
    /// own `EnvGuard` both acquire it, on a lock that is not reentrant, so a
    /// caller cannot hold it across both. This lock is always taken **before**
    /// `ENV_LOCK` and released after it, never the other way round.
    ///
    /// Poison-tolerant on purpose (same rule as `ENV_LOCK`, #1788): the
    /// poisoner asserts while holding the window, and a failed assertion must
    /// not cascade into every later test in the process.
    #[must_use = "the guard is the window; dropping it reopens the surface for sibling parses"]
    pub(crate) fn clap_env_window() -> std::sync::MutexGuard<'static, ()> {
        static WINDOW: std::sync::Mutex<()> = std::sync::Mutex::new(());
        WINDOW
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Every non-builtin clap arg must have an `OptionSpec` entry.
    pub(crate) fn assert_surface_covered(args: &[clap::Arg], group: &[OptionSpec]) {
        assert_surface_covered_except(args, group, &[]);
    }

    /// Same as [`assert_surface_covered`], ignoring hand-built slots that
    /// are recorded in the spec but not routed through `build_arg`
    /// (crawler's deferred `concurrency` / pattern / header args). Prefer
    /// this over a local coverage loop: the genuine difference is the
    /// exclusion list, not the assertion.
    pub(crate) fn assert_surface_covered_except(
        args: &[clap::Arg],
        group: &[OptionSpec],
        manual_ids: &[&str],
    ) {
        for arg in args {
            let id = arg.get_id().as_str();
            if matches!(id, "help" | "version") || manual_ids.contains(&id) {
                continue;
            }
            assert!(
                group.iter().any(|s| s.id == id),
                "clap arg `{id}` has no OptionsSpec entry — spec is out of sync",
            );
        }
    }

    /// Long, short, aliases, env, and help heading must match the spec.
    /// Specs gated off in this build are skipped: they have no clap arg
    /// to check, and their surface is pinned separately.
    pub(crate) fn assert_long_short_alias_env_heading(args: &[clap::Arg], group: &[OptionSpec]) {
        for s in group {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(args, s.id, "spec group");
            assert_eq!(arg.get_long(), Some(s.long), "long mismatch for `{}`", s.id);
            assert_eq!(arg.get_short(), s.short, "short mismatch for `{}`", s.id);
            let aliases = arg.get_aliases().unwrap_or_default();
            assert_eq!(aliases, s.aliases, "alias mismatch for `{}`", s.id);
            let env = arg.get_env().map(|e| e.to_string_lossy().into_owned());
            assert_eq!(env.as_deref(), s.env, "env var mismatch for `{}`", s.id);
            assert_eq!(
                arg.get_help_heading(),
                s.heading,
                "help heading mismatch for `{}`",
                s.id
            );
        }
    }

    /// Clap defaults must match the spec's canonical defaults.
    /// Specs gated off in this build are skipped (see
    /// [`assert_long_short_alias_env_heading`]).
    pub(crate) fn assert_defaults(args: &[clap::Arg], group: &[OptionSpec]) {
        for s in group {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(args, s.id, "spec group");
            let defaults: Vec<String> = arg
                .get_default_values()
                .iter()
                .map(|v| v.to_string_lossy().into_owned())
                .collect();
            let expected: Vec<String> = s.default.map(|d| vec![d.to_string()]).unwrap_or_default();
            assert_eq!(defaults, expected, "default mismatch for `{}`", s.id);
        }
    }

    /// Help text must match the spec verbatim.
    /// Specs gated off in this build are skipped (see
    /// [`assert_long_short_alias_env_heading`]).
    pub(crate) fn assert_help(args: &[clap::Arg], group: &[OptionSpec]) {
        for s in group {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(args, s.id, "spec group");
            let help = arg
                .get_long_help()
                .or_else(|| arg.get_help())
                .unwrap_or_else(|| panic!("arg `{}` has no help text", s.id))
                .to_string();
            assert_eq!(
                help.trim(),
                s.help.trim(),
                "help text mismatch for `{}`",
                s.id
            );
        }
    }

    /// Structural clap surface must match the spec: action per value kind,
    /// SCREAMING value name, possible values for enums only, delimiter
    /// round-trip, and no long help. Specs gated off in this build are
    /// skipped (see [`assert_long_short_alias_env_heading`]).
    pub(crate) fn assert_structural(args: &[clap::Arg], group: &[OptionSpec]) {
        for s in group {
            if !s.active() {
                continue; // gated off in this build: placeholder pinned separately
            }
            let arg = arg_by_id(args, s.id, "spec group");
            assert_structural_one(arg, s);
        }
    }

    /// Structural check for a single spec behind [`assert_structural`],
    /// exposed so groups with a genuinely special arity (crawler's
    /// occurrence-counted `-v`) can keep their own dispatch loop for that
    /// one arg and still share the body for everything else.
    pub(crate) fn assert_structural_one(arg: &clap::Arg, s: &OptionSpec) {
        assert_structural_action(arg, s);
        assert_structural_shape(arg, s);
    }

    /// Action dispatch behind [`assert_structural_one`]: bools take
    /// `SetTrue`, text lists `Append`, everything else `Set`.
    pub(crate) fn assert_structural_action(arg: &clap::Arg, s: &OptionSpec) {
        use crate::domain::options_spec::ValueKind;
        match s.kind {
            ValueKind::Bool => {
                assert!(
                    matches!(arg.get_action(), clap::ArgAction::SetTrue),
                    "bool `{}` must use SetTrue",
                    s.id
                );
            },
            // A `TextList` (`Option<Vec<String>>` derive) resolves to
            // `ArgAction::Append`, so repeated occurrences append. The
            // spec builder mirrors that exactly.
            ValueKind::TextList => {
                assert!(
                    matches!(arg.get_action(), clap::ArgAction::Append),
                    "text list `{}` must use Append",
                    s.id
                );
            },
            _ => {
                assert!(
                    matches!(arg.get_action(), clap::ArgAction::Set),
                    "value option `{}` must use Set",
                    s.id
                );
            },
        }
    }

    /// Value-name / possible-values / delimiter / long-help shape behind
    /// [`assert_structural_one`].
    pub(crate) fn assert_structural_shape(arg: &clap::Arg, s: &OptionSpec) {
        use crate::domain::options_spec::ValueKind;
        let names: Vec<String> = arg
            .get_value_names()
            .unwrap_or_default()
            .iter()
            .map(|id| id.to_string())
            .collect();
        assert_eq!(
            names,
            vec![s.id.to_ascii_uppercase()],
            "value name mismatch for `{}`",
            s.id
        );
        let possible: Vec<String> = arg
            .get_possible_values()
            .into_iter()
            .map(|v| v.get_name().to_string())
            .collect();
        if let ValueKind::Enum { variants } = s.kind {
            assert_eq!(possible, variants, "possible values for `{}`", s.id);
        } else if !matches!(s.kind, ValueKind::Bool) {
            assert!(
                possible.is_empty(),
                "`{}` must have no possible values",
                s.id
            );
        }
        assert_eq!(
            arg.get_value_delimiter(),
            s.value_delimiter,
            "value_delimiter mismatch for `{}`",
            s.id
        );
        assert!(
            arg.get_long_help().is_none(),
            "`{}` must not carry long help",
            s.id
        );
    }

    /// Runs `f` with every ambient environment variable that could leak into
    /// clap's `env` fallbacks (`WEBFANG_*`, `AI_MODEL_ID`) temporarily
    /// removed, restoring them afterwards. Callers hold [`clap_env_window`].
    ///
    /// Hermeticity: several args declare `env = "WEBFANG_..."` fallbacks, so a
    /// bare parse absorbs ambient variables and default assertions fail in
    /// environments that export them (the scheduled bug-discovery job poisons
    /// exactly these names — issue #926). Tests must observe the declared
    /// defaults, not the ambient shell.
    fn with_clap_env_cleared<T>(f: impl FnOnce() -> T) -> T {
        // Snapshot the matching keys while holding ENV_LOCK so the iteration
        // cannot race a concurrent mutation (issue #1126); the guard then
        // removes them and restores on drop.
        let keys: Vec<String> = {
            let _lock = webfang_test_utils::env_lock();
            std::env::vars()
                .filter(|(name, _)| name.starts_with("WEBFANG_") || name == "AI_MODEL_ID")
                .map(|(name, _)| name)
                .collect()
        };
        let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let _guard = webfang_test_utils::EnvGuard::clean(&refs);
        f()
    }
}

/// CLI Arguments for the webfang binary.
///
/// Parsed using `clap` with derive macros.
///
/// # Examples
///
/// ```no_run
/// use webfang_core::Args;
/// use clap::Parser;
///
/// let args = Args::parse_from([
///     "webfang",
///     "--url", "https://example.com",
///     "--output", "./output",
///     "--export-format", "jsonl",
///     "--resume",
/// ]);
///
/// assert_eq!(
///     args.crawler.url.as_ref().map(webfang_core::domain::ValidUrl::as_str),
///     Some("https://example.com")
/// );
/// ```
#[derive(Parser, Debug, Default)]
#[command(name = "webfang", version)]
#[command(
    about = "High-performance web scraper with WAF evasion and AI-powered content cleaning",
    after_help = "EXIT CODES:\n  0    Success\n  2    No URLs discovered\n  3    All scrapers failed\n  64   Bad CLI arguments (usage error)\n  65   Data format error (extraction without usable content)\n  69   Network error, partial success, or SSRF refusal\n  74   I/O error\n  76   Protocol error\n  77   All URLs blocked by robots.txt\n  78   Configuration error\n\nEXAMPLES:\n  webfang https://example.com\n  webfang -u https://example.com\n  webfang -u https://example.com --ai\n  webfang -u https://example.com -f jsonl\n  webfang -u https://example.com -v\n  webfang -u https://example.com -vv  # DEBUG\n  webfang --batch-file urls.txt --resume"
)]
#[command(subcommand_negates_reqs = true)]
pub struct Args {
    /// Subcommands
    #[command(subcommand)]
    pub subcommand: Option<Commands>,

    /// URL to scrape (positional shorthand — equivalent to --url). Hardened
    /// through the SAME argv-boundary parser as `--url` (#1239): the
    /// shorthand cannot bypass the credential strip.
    #[arg(value_name = "URL", conflicts_with = "url", value_parser = crate::cli::args::crawler::parse_seed_url)]
    pub positional_url: Option<ValidUrl>,

    /// Crawler and discovery configuration.
    #[command(flatten)]
    pub crawler: CrawlerArgs,

    /// Export format and output configuration.
    #[command(flatten)]
    pub export: ExportArgs,

    /// Obsidian vault integration settings.
    #[command(flatten)]
    pub obsidian: ObsidianArgs,

    /// AI-powered semantic cleaning settings.
    #[command(flatten)]
    pub ai: AiArgs,

    /// LLM extraction provider settings.
    #[command(flatten)]
    pub llm: LlmArgs,
}

mod commands;
mod crawl_options_map;

pub use commands::Commands;

/// Carrier for the values `doctor` / `show` read, so diagnostics can name
/// their provenance instead of emitting anonymous numbers.
pub struct SubcommandArgs {
    /// The provider id the operator named via `--embedding-provider`, if any.
    ///
    /// Both `doctor` and `show` resolve it as an embeddings-provider id — the
    /// same provider the scrape path would dial. The model-profile namespace
    /// lives in the `[[model_profiles]]` config block, not in this flag.
    pub provider_id: Option<String>,
    /// Whether the operator chose `--max-chars` (or `WEBFANG_MAX_CHARS`).
    pub max_chars_explicit: bool,
    /// The effective `--max-chars` value.
    pub max_chars_value: Option<usize>,
    /// Whether the operator chose `--threshold`.
    pub threshold_explicit: bool,
    /// The effective `--threshold` value.
    pub threshold_value: Option<f32>,
}

/// Shell type for completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Shell {
    /// Bash shell completions.
    Bash,
    /// Elvish shell completions.
    Elvish,
    /// Fish shell completions.
    Fish,
    /// PowerShell completions.
    PowerShell,
    /// Zsh shell completions.
    Zsh,
}

impl From<Shell> for clap_complete::Shell {
    fn from(s: Shell) -> Self {
        match s {
            Shell::Bash => clap_complete::Shell::Bash,
            Shell::Elvish => clap_complete::Shell::Elvish,
            Shell::Fish => clap_complete::Shell::Fish,
            Shell::PowerShell => clap_complete::Shell::PowerShell,
            Shell::Zsh => clap_complete::Shell::Zsh,
        }
    }
}

impl Args {
    /// Build [`ElasticOverrides`] (PR5) from the elastic-ingestion CLI flags.
    ///
    /// `--ram-budget` is already parsed to bytes by the clap value parser, which
    /// accepts suffixed values (`8GB`, `2048MB`, plain bytes) and rejects zero
    /// or malformed input at the boundary (#653). The result feeds
    /// [`ElasticConfig::resolve`] → Rayon pool size, byte-weighted semaphore,
    /// and SQLite path.
    ///
    /// [`ElasticConfig::resolve`]: crate::infrastructure::autotuning::ElasticConfig::resolve
    /// [`ElasticOverrides`]: crate::infrastructure::autotuning::ElasticOverrides
    #[must_use]
    pub fn elastic_overrides(&self) -> crate::infrastructure::autotuning::ElasticOverrides {
        use crate::domain::config::ElasticOverrides;
        ElasticOverrides {
            cpu_cores: self.export.cpu_cores,
            ram_budget_bytes: self.export.ram_budget,
            max_resource_bytes: None,
            db_path: self.export.db_path.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // #2234 / #2341 — the crate's test parse surface must be hermetic, and no
    // sibling parse may observe another test's poisoned window.
    //
    // The bug-discovery lane poisons the environment on purpose
    // (`WEBFANG_URL=POISON_TRASH` and friends). A test that parses through
    // a non-hermetic helper absorbs that poison: the bare parse dies with
    // "invalid value 'POISON_TRASH' for '--url <URL>'" on the lane while
    // passing on a developer machine — a test that depends on the shell of
    // whoever runs it.
    //
    // #2341: hermeticity alone was not enough. This test mutates the process
    // environment for a window, and libtest runs the sibling parsers
    // concurrently in ONE process (the Coverage lane's `cargo llvm-cov`), so a
    // raw `Args::try_parse_from` could read the poison here. The window lock
    // spans `set → parse → restore`, and every parse in this module takes it,
    // so the poison is observable only by the parse that means to observe it.
    //
    // The env lock is NOT reentrant: `EnvGuard` holds it while alive and the
    // parse helper takes it again, so holding a guard across the parse
    // deadlocks. The poison is therefore set and removed with the transient
    // helpers (`env_set` / `env_remove`), which take the lock only for the
    // duration of the mutation.
    // ========================================================================

    #[test]
    fn the_test_parse_surface_ignores_the_ambient_environment() {
        // Snapshot the ambient value (if any) so the test leaves the
        // environment exactly as it found it. Snapshot before taking the
        // window: `env_lock` is only held for the read.
        let original = {
            let _lock = webfang_test_utils::env_lock();
            std::env::var("WEBFANG_URL").ok()
        };

        let window = test_support::clap_env_window();
        webfang_test_utils::env_set("WEBFANG_URL", "POISON_TRASH");
        // Capture the outcome BEFORE restoring the environment: the
        // assertion runs against the parse that saw the poisoned env.
        let parsed = test_support::parse_args_hermetic_in_window(&[]);
        match original {
            Some(value) => webfang_test_utils::env_set("WEBFANG_URL", &value),
            None => webfang_test_utils::env_remove("WEBFANG_URL"),
        }
        drop(window);

        let args = parsed.expect("bare invocation must parse despite the poisoned environment");
        assert!(
            args.crawler.url.is_none(),
            "the ambient WEBFANG_URL must not reach the parsed args"
        );
        assert!(
            args.positional_url.is_none(),
            "the ambient WEBFANG_URL must not reach the positional URL slot"
        );
    }

    // ========================================================================
    // TASK-13 — --ignore-waf flag + propagation (REQ-WAF-07)
    // ========================================================================

    #[test]
    fn ignore_waf_flag_defaults_to_false() {
        let args =
            test_support::parse_args_hermetic(&["-u", "https://example.com"]).expect("valid args");
        assert!(!args.crawler.ignore_waf, "ignore_waf defaults to false");
    }

    #[test]
    fn ignore_waf_flag_parses() {
        let args =
            test_support::parse_args_hermetic(&["-u", "https://example.com", "--ignore-waf"])
                .expect("valid args");
        assert!(args.crawler.ignore_waf, "--ignore-waf sets the flag");
    }

    #[test]
    fn ignore_waf_maps_into_crawl_options() {
        let args =
            test_support::parse_args_hermetic(&["-u", "https://example.com", "--ignore-waf"])
                .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert!(
            opts.crawl.ignore_waf,
            "ignore_waf must propagate Args -> CrawlOptions"
        );
    }

    // ========================================================================
    // #1813 — the burst is validated by ONE authority, and `From<Args>` is
    // no longer a second, panicking one.
    //
    // Pre-fix this test was `#[should_panic(expected = "--rate-limit-burst…")]`
    // and proved the WRONG thing: production code panicking on user input.
    // `normalize()` (`cli::preflight::stage_budget_overrides`) already
    // rejected `0` with `CliExit::ConfigError` (exit 78) BEFORE this
    // conversion runs, and the validated value travels forward through
    // `merge_budget_overrides`. So the correct invariant for an infallible
    // `From` is: never panic, and never smuggle an unvalidated value.
    // ========================================================================

    #[test]
    fn from_args_never_panics_on_invalid_rate_limit_burst() {
        for raw in ["0", "banana", "4294967296"] {
            let args = test_support::parse_args_hermetic(&[
                "-u",
                "https://example.com",
                "--rate-limit-burst",
                raw,
            ])
            .expect("clap accepts the raw string; validation happens in preflight");

            // The whole point: an invalid burst must not abort the process.
            let opts = crate::application::crawl_options::CrawlOptions::from(args);
            assert_eq!(
                opts.budget_overrides.rate_burst, None,
                "`From<Args>` must not smuggle an unvalidated burst for {raw}; \
                 the value is owned by the preflight pipeline"
            );
        }
    }

    /// A VALID burst survives the same infallible `From` — as `None`, i.e.
    /// deliberately deferred. The pipeline test in `cli::preflight`
    /// (`main_flow_merge_preserves_valid_burst_override`) pins that it is then
    /// supplied by `normalize()` + `merge_budget_overrides`, so this pair
    /// together proves the override is never lost.
    #[test]
    fn from_args_defers_valid_rate_limit_burst_to_the_pipeline() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--rate-limit-burst",
            "7",
        ])
        .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            opts.budget_overrides.rate_burst, None,
            "the pipeline owns burst resolution; `From<Args>` contributes nothing here"
        );
    }

    // ========================================================================
    // #344 — Positional URL argument
    // ========================================================================

    #[test]
    fn positional_url_parses() {
        let args = test_support::parse_args_hermetic(&["https://example.com"]).expect("valid args");
        assert_eq!(
            args.positional_url.as_ref().map(ValidUrl::as_str),
            Some("https://example.com/"),
            "positional URL captured (trailing slash added by ValidUrl)"
        );
    }

    #[test]
    fn positional_url_with_flags() {
        let args = test_support::parse_args_hermetic(&["https://example.com", "--max-pages", "5"])
            .expect("valid args");
        assert_eq!(
            args.positional_url.as_ref().map(ValidUrl::as_str),
            Some("https://example.com/")
        );
        assert_eq!(args.crawler.max_pages, 5);
    }

    #[test]
    fn positional_url_conflicts_with_flag() {
        let result =
            test_support::parse_args_hermetic(&["https://example.com", "-u", "https://other.com"]);
        assert!(result.is_err(), "positional + -u must conflict");
    }

    #[test]
    fn completions_still_work() {
        let args = test_support::parse_args_hermetic(&["completions", "bash"]).expect("valid args");
        assert!(
            matches!(
                args.subcommand,
                Some(Commands::Completions { shell: Shell::Bash })
            ),
            "completions subcommand parses without URL"
        );
    }

    #[test]
    fn no_args_no_url_is_ok() {
        let args = test_support::parse_args_hermetic(&[]).expect("valid args");
        assert!(args.positional_url.is_none());
        assert!(args.crawler.url.is_none());
    }

    // ========================================================================
    // #640 — --batch-concurrency 0 must be rejected at parse time (exit 64)
    // ========================================================================

    #[test]
    fn batch_concurrency_zero_is_rejected() {
        let result = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--batch-concurrency",
            "0",
        ]);
        assert!(
            result.is_err(),
            "--batch-concurrency 0 must fail clap validation, not panic at runtime"
        );
    }

    #[test]
    fn batch_concurrency_positive_is_accepted() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--batch-concurrency",
            "3",
        ])
        .expect("valid args");
        assert_eq!(args.export.batch_concurrency, Some(3));
    }

    #[test]
    fn batch_concurrency_omitted_is_none_auto() {
        let args =
            test_support::parse_args_hermetic(&["-u", "https://example.com"]).expect("valid args");
        // Omitted flag = auto: the budget model derives the tier.
        assert_eq!(args.export.batch_concurrency, None);
    }

    // ========================================================================
    // #675 — --cookie and --header flags
    // ========================================================================

    #[test]
    fn header_flag_parses_single() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.headers,
            vec!["Authorization: Bearer TOKEN".to_string()]
        );
    }

    #[test]
    fn header_flag_parses_multiple() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
            "-H",
            "X-Custom: value",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.headers,
            vec![
                "Authorization: Bearer TOKEN".to_string(),
                "X-Custom: value".to_string()
            ]
        );
    }

    #[test]
    fn cookie_flag_parses_single() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
        ])
        .expect("valid args");
        assert_eq!(args.crawler.cookies, vec!["session=abc123".to_string()]);
    }

    #[test]
    fn cookie_flag_parses_multiple() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
            "--cookie",
            "csrf=xyz",
        ])
        .expect("valid args");
        assert_eq!(
            args.crawler.cookies,
            vec!["session=abc123".to_string(), "csrf=xyz".to_string()]
        );
    }

    #[test]
    fn headers_propagate_to_crawl_options() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "-H",
            "Authorization: Bearer TOKEN",
        ])
        .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            opts.network.custom_headers,
            vec![("Authorization".to_string(), "Bearer TOKEN".to_string())]
        );
    }

    #[test]
    fn cookies_propagate_to_crawl_options() {
        let args = test_support::parse_args_hermetic(&[
            "-u",
            "https://example.com",
            "--cookie",
            "session=abc123",
        ])
        .expect("valid args");
        let opts = crate::application::crawl_options::CrawlOptions::from(args);
        assert_eq!(
            opts.network.initial_cookies,
            vec![("session".to_string(), "abc123".to_string())]
        );
    }
}
