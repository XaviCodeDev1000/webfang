//! Normalization stages: env/CLI merge.

use tracing::instrument;

use crate::Args;

use super::field_book::{ArgSources, FieldBook};
use super::normalization::try_write;

fn stage_env_cli_crawl(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("max_pages") {
        if try_write(
            &mut book.max_pages,
            args.crawler.max_pages,
            src,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("max_depth") {
        if try_write(
            &mut book.max_depth,
            args.crawler.max_depth,
            src,
            "max_depth",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("delay_ms") {
        if try_write(&mut book.delay_ms, args.crawler.delay_ms, src, "delay_ms") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("jitter_ms") {
        if try_write(
            &mut book.jitter_ms,
            args.crawler.jitter_ms,
            src,
            "jitter_ms",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("timeout_secs") {
        if try_write(
            &mut book.timeout_secs,
            args.crawler.timeout_secs,
            src,
            "timeout_secs",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("selector") {
        if try_write(
            &mut book.selector,
            args.crawler.selector.clone(),
            src,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("concurrency") {
        if try_write(
            &mut book.concurrency,
            args.crawler.concurrency.clone(),
            src,
            "concurrency",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("sitemap_url") {
        let val = args.crawler.sitemap_url.clone();
        if try_write(&mut book.sitemap_url, val, src, "sitemap_url") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("use_sitemap") {
        if try_write(
            &mut book.use_sitemap,
            args.crawler.use_sitemap,
            src,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_output(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("output") {
        if try_write(&mut book.output, args.export.output.clone(), src, "output") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("format") {
        if try_write(&mut book.format, args.export.format, src, "format") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("export_format") {
        if try_write(
            &mut book.export_format,
            args.export.export_format,
            src,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_env_cli_obsidian(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    if let Some(src) = sources.source_of("obsidian_tags") {
        if try_write(
            &mut book.obsidian_tags,
            args.obsidian.obsidian_tags.clone().unwrap_or_default(),
            src,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_wiki_links") {
        if try_write(
            &mut book.obsidian_wiki_links,
            args.obsidian.obsidian_wiki_links,
            src,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("obsidian_relative_assets") {
        if try_write(
            &mut book.obsidian_relative_assets,
            args.obsidian.obsidian_relative_assets,
            src,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("vault") {
        if try_write(&mut book.vault, args.obsidian.vault.clone(), src, "vault") {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("quick_save") {
        if try_write(
            &mut book.quick_save,
            args.obsidian.quick_save,
            src,
            "quick_save",
        ) {
            n += 1;
        }
    }
    if let Some(src) = sources.source_of("ignore_waf") {
        if try_write(
            &mut book.ignore_waf,
            args.crawler.ignore_waf,
            src,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    n
}

#[instrument(skip_all, fields(fields_written))]
pub(super) fn stage_env_cli(book: &mut FieldBook, args: &Args, sources: &ArgSources) -> usize {
    let mut n = 0;
    n += stage_env_cli_crawl(book, args, sources);
    n += stage_env_cli_output(book, args, sources);
    n += stage_env_cli_obsidian(book, args, sources);
    n
}
