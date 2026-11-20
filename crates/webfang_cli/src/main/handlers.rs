//! Subcommand handlers for `webfang connect`, `doctor`, `show` and `recover`.

use super::*;

/// Handle the `connect` subcommand (issue #1933, ADR-0004 step 4).
///
/// The variant carries the endpoint, the required model, and an optional
/// provider id. With no id, a `connect-<hex>` name is generated from the URL
/// so repeated runs against the same endpoint are idempotent-ish and readable.
pub(super) async fn handle_connect(args: &Args) -> CliExit {
    let Some(Commands::Connect { url, model, id }) = args.subcommand.as_ref() else {
        return CliExit::UsageError("connect invocado sin sus argumentos".into());
    };
    let id = id
        .clone()
        .unwrap_or_else(|| generated_provider_id(url.as_str()));
    let config_path = resolve_config_path();
    webfang_core::cli::commands::connect::run(url.as_url(), &id, model, &config_path).await
}

/// Handle the `doctor` subcommand: diagnose the embedding path from config.
pub(super) async fn handle_doctor(args: &Args) -> CliExit {
    let config_path = resolve_config_path();
    let sub = subcommand_args(args);
    webfang_core::cli::commands::doctor::run(&config_path, &sub).await
}

/// Handle the `show` subcommand: print the effective profile + budget.
pub(super) fn handle_show(args: &Args) -> CliExit {
    let config_path = resolve_config_path();
    let sub = subcommand_args(args);
    webfang_core::cli::commands::show::run(&sub, &config_path)
}

/// Handle the `recover` subcommand (#2611): convert a stranded batch capture
/// spool into the `.md` files its clean close never wrote. Runs before any
/// config/preflight like the other subcommands — it consumes no config.
pub(super) async fn handle_recover(args: &Args) -> CliExit {
    let Some(Commands::Recover {
        spool,
        output,
        strict,
    }) = args.subcommand.as_ref()
    else {
        return CliExit::UsageError("recover invocado sin sus argumentos".into());
    };
    webfang_core::cli::commands::recover::run(spool, output, *strict).await
}
