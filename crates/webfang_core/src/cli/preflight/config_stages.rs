//! Normalization stages: config file merge.

use std::path::PathBuf;

use crate::cli::config::ConfigDefaults;
use crate::domain::config_value::ConfigSource;
use crate::{ConcurrencyConfig, ExportFormat, OutputFormat};

use super::field_book::FieldBook;
use tracing::instrument;

use super::normalization::try_write;

#[instrument(skip_all, fields(fields_written))]
pub(super) fn stage_config_file(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    n += stage_config_file_crawl(book, config);
    n += stage_config_file_output(book, config);
    n += stage_config_file_obsidian(book, config);
    n
}

fn stage_config_file_crawl(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.max_pages {
        if try_write(
            &mut book.max_pages,
            v,
            ConfigSource::ConfigFile,
            "max_pages",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.delay_ms {
        if try_write(&mut book.delay_ms, v, ConfigSource::ConfigFile, "delay_ms") {
            n += 1;
        }
    }
    if let Some(v) = config.jitter_ms {
        if try_write(
            &mut book.jitter_ms,
            v,
            ConfigSource::ConfigFile,
            "jitter_ms",
        ) {
            n += 1;
        }
    }
    if let Some(ref s) = config.selector {
        if try_write(
            &mut book.selector,
            s.clone(),
            ConfigSource::ConfigFile,
            "selector",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.use_sitemap {
        if try_write(
            &mut book.use_sitemap,
            v,
            ConfigSource::ConfigFile,
            "use_sitemap",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.ignore_waf {
        if try_write(
            &mut book.ignore_waf,
            v,
            ConfigSource::ConfigFile,
            "ignore_waf",
        ) {
            n += 1;
        }
    }
    // concurrency via string tag (mirrors apply_config_defaults)
    if let Some(ref c) = config.concurrency {
        let target = ConcurrencyConfig::from(c.as_str());
        // enact rank guard even when target equals default 'auto'
        if try_write(
            &mut book.concurrency,
            target,
            ConfigSource::ConfigFile,
            "concurrency",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_output(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(ref fmt) = config.format {
        let target = match fmt.to_lowercase().as_str() {
            "json" => OutputFormat::Json,
            "text" => OutputFormat::Text,
            _ => OutputFormat::Markdown,
        };
        if try_write(&mut book.format, target, ConfigSource::ConfigFile, "format") {
            n += 1;
        }
    }
    if let Some(ref fmt) = config.export_format {
        let target = match fmt.to_lowercase().as_str() {
            "vector" => ExportFormat::Vector,
            "auto" => ExportFormat::Auto,
            _ => ExportFormat::Jsonl,
        };
        if try_write(
            &mut book.export_format,
            target,
            ConfigSource::ConfigFile,
            "export_format",
        ) {
            n += 1;
        }
    }
    n
}

fn stage_config_file_obsidian(book: &mut FieldBook, config: &ConfigDefaults) -> usize {
    let mut n = 0;
    if let Some(v) = config.obsidian_wiki_links {
        if try_write(
            &mut book.obsidian_wiki_links,
            v,
            ConfigSource::ConfigFile,
            "obsidian_wiki_links",
        ) {
            n += 1;
        }
    }
    if let Some(v) = config.obsidian_relative_assets {
        if try_write(
            &mut book.obsidian_relative_assets,
            v,
            ConfigSource::ConfigFile,
            "obsidian_relative_assets",
        ) {
            n += 1;
        }
    }
    if let Some(ref vault) = config.vault_path {
        if try_write(
            &mut book.vault,
            Some(PathBuf::from(vault)),
            ConfigSource::ConfigFile,
            "vault",
        ) {
            n += 1;
        }
    }
    // Rank-guarded by `try_write` alone: at this fixed pipeline position the
    // slot can only be `Default`-sourced, so value-emptiness must not gate
    // the write (spec R2 forbids value-equality merge logic).
    if let Some(ref tags_str) = config.obsidian_tags {
        let parsed: Vec<String> = tags_str
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect();
        if try_write(
            &mut book.obsidian_tags,
            parsed,
            ConfigSource::ConfigFile,
            "obsidian_tags",
        ) {
            n += 1;
        }
    }
    n
}
