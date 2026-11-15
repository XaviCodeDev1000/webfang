//! Exporter factory: builds the right [`Exporter`] for an [`ExportFormat`]
//! and an output directory, including `Auto` detection over existing files.
//! Split out of `export_factory.rs` under the ~500-line rule (#2584, Ola 1);
//! mechanical movement only.

use std::path::PathBuf;

use tracing::info;

use crate::domain::crawler_port::filename::confine_filename_component;
use crate::domain::entities::ExportFormat;
use crate::domain::exporter::ExporterError;
use crate::domain::{Exporter, ExporterConfig};

/// Map an export-stage failure onto the SC6 taxonomy: item-data problems are
/// `DomainRecoverable` (record + continue); environment failures stay fatal.
pub(crate) fn classify_export_failure(err: &ExporterError) -> crate::error::ErrorClass {
    use crate::error::ErrorClass;
    match err {
        ExporterError::Serialization(_) | ExporterError::InvalidConfig(_) => {
            ErrorClass::DomainRecoverable
        },
        _ => ErrorClass::InternalFatal,
    }
}

/// Create exporter based on output format
pub fn create_exporter(
    output_dir: PathBuf,
    filename: &str,
    format: ExportFormat,
) -> Result<Box<dyn Exporter>, ExporterError> {
    match format {
        ExportFormat::Jsonl => Ok(create_jsonl_exporter(output_dir, filename)),
        ExportFormat::Vector => Ok(create_vector_exporter(output_dir, filename)),
        ExportFormat::Auto => create_auto_exporter(output_dir, filename),
    }
}

/// Build a JSONL exporter with append mode enabled.
fn create_jsonl_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Jsonl, filename).with_append(true);
    info!("Creating JSONL exporter: {:?}", config.output_path());
    crate::application::container::build_jsonl_exporter(config)
}

/// Build a Vector exporter with append mode enabled.
fn create_vector_exporter(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    let config = ExporterConfig::new(output_dir, ExportFormat::Vector, filename).with_append(true);
    info!("Creating Vector exporter: {:?}", config.output_path());
    crate::application::container::build_vector_exporter(config)
}

/// Auto-detect the export format from existing files (`export.jsonl` /
/// `export.json`), falling back to JSONL when neither exists.
fn create_auto_exporter(
    output_dir: PathBuf,
    filename: &str,
) -> Result<Box<dyn Exporter>, ExporterError> {
    // Auto-detect: checks if export.jsonl or export.json exists
    info!("Auto-detecting format...");

    // Confine before probing so a hostile filename can never escape
    // `output_dir` through the existence checks below (#1125).
    let filename = confine_filename_component(filename, "export");
    let jsonl_path = output_dir.join(format!("{filename}.jsonl"));
    let vector_path = output_dir.join(format!("{filename}.json"));

    if jsonl_path.exists() {
        Ok(create_detected_jsonl(output_dir, &filename, &jsonl_path))
    } else if vector_path.exists() {
        Ok(create_detected_vector(output_dir, &filename, &vector_path))
    } else {
        Ok(create_default_jsonl(output_dir, &filename))
    }
}

/// Build a JSONL exporter for the auto-detected `.jsonl` file.
fn create_detected_jsonl(
    output_dir: PathBuf,
    filename: &str,
    jsonl_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected JSONL format - {:?} exists", jsonl_path);
    create_jsonl_exporter(output_dir, filename)
}

/// Build a Vector exporter for the auto-detected `.json` file.
fn create_detected_vector(
    output_dir: PathBuf,
    filename: &str,
    vector_path: &std::path::Path,
) -> Box<dyn Exporter> {
    info!("Detected Vector format - {:?} exists", vector_path);
    create_vector_exporter(output_dir, filename)
}

/// Build a JSONL exporter when no existing export file is found.
fn create_default_jsonl(output_dir: PathBuf, filename: &str) -> Box<dyn Exporter> {
    info!("No existing export, using default Jsonl format");
    create_jsonl_exporter(output_dir, filename)
}
