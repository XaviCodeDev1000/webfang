//! `webfang` subcommands.

use crate::domain::ValidUrl;

use super::Shell;

/// Subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    /// Generate shell completion scripts
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Detect and validate a remote embedding endpoint, then write its config
    Connect {
        /// Base URL of the OpenAI-compatible embeddings endpoint.
        #[arg(value_name = "URL", value_parser = crate::cli::args::crawler::parse_seed_url)]
        url: ValidUrl,
        /// Model id the endpoint serves. Required: the remote adapter refuses
        /// to build without one (the endpoint demands it on every request).
        #[arg(long, value_name = "MODEL")]
        model: String,
        /// Provider id to register. Defaults to a generated `connect-<hex>`.
        #[arg(long, value_name = "ID")]
        id: Option<String>,
    },
    /// Diagnose the embedding path — config, credential, network, budget
    Doctor,
    /// Dump the effective embeddings provider, the configured budget, and the
    /// historical model-profile metadata
    ///
    /// The provider is what a run actually uses; the profile metadata is
    /// historical (ADR-0004 step 4) and no run consumes it.
    Show,
    /// Recover pages from a stranded batch capture spool into markdown files
    ///
    /// A batch killed before its clean close leaves every captured page in the
    /// raw `.webfang-batch-capture.jsonl` spool with ZERO `.md` files on disk
    /// (#2611). This command converts every complete record through the SAME
    /// extraction and writer pipeline the batch export uses — same filenames,
    /// same frontmatter, atomic per file.
    ///
    /// Note: extraction options come from the flags below ONLY. The options of
    /// the original run are not recorded in the spool, so output can diverge
    /// from what that run would have written if it used a non-default
    /// `--selector` or `--ignore-waf`.
    Recover {
        /// Path to the stranded capture spool (`.webfang-batch-capture.jsonl`)
        #[arg(long, value_name = "FILE")]
        spool: std::path::PathBuf,
        /// Directory the recovered `.md` files are written to
        #[arg(short = 'o', long, value_name = "DIR")]
        output: std::path::PathBuf,
        /// Exit non-zero when any record is corrupt or fails extraction
        /// (default: report them and exit 0 as long as something converted)
        #[arg(long)]
        strict: bool,
    },
}
