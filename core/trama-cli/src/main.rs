// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Compile source network data into TRAMA files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::Value;
use trama_format::{Import, Importer, parse_options, read_sections};

/// Formats the core does not know are read by the crates that own them. The seam is the same
/// one a plugin would use; linking them here only decides which are present in this binary.
fn importers() -> Vec<Box<dyn Importer>> {
    vec![Box::new(trama_epanet::importer::EpanetImporter)]
}

const NATIVE_SUFFIXES: [&str; 2] = [".geojson", ".json"];

#[derive(Parser)]
#[command(name = "trama", about = "Compile source network data into TRAMA files.")]
struct Arguments {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compile GeoJSON, or any format an importer claims, into a `.trama` file.
    Compile {
        source: PathBuf,
        destination: PathBuf,
        /// JSON list of state channels to declare.
        #[arg(long)]
        channels: Option<PathBuf>,
        /// key=value passed to the importer.
        #[arg(long = "option", short = 'o')]
        options: Vec<String>,
    },
    /// Validate a `.trama` container.
    Validate { source: PathBuf },
    /// Export a `.trama` file into a directory of GeoJSON FeatureCollections.
    Export {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long, default_value = "geojson")]
        to: String,
        /// Required by `--to inp`: the coordinate reference system to write back into.
        #[arg(long = "option", short = 'o')]
        options: Vec<String>,
    },
}

fn main() -> ExitCode {
    match run(Arguments::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> Result<(), String> {
    match arguments.command {
        Command::Compile { source, destination, channels, options } => {
            let declared: Vec<Value> = match &channels {
                Some(path) => serde_json::from_str(&read(path)?).map_err(|error| error.to_string())?,
                None => Vec::new(),
            };
            let options = parse_options(&options)?;
            let imported = load(&source, &options)?;
            // An explicit --channels wins: the caller may know more than the format does.
            let channels = if declared.is_empty() { &imported.channels } else { &declared };
            let bytes = trama_format::compile(&imported.features, channels, &imported.extras)?;
            std::fs::write(&destination, bytes).map_err(|error| error.to_string())
        }
        Command::Validate { source } => read_sections(&bytes(&source)?).map(|_sections| ()),
        Command::Export { source, destination, to, options } => {
            let container = bytes(&source)?;
            match to.as_str() {
                "geojson" => {
                    let exported = trama_format::export(&container)?;
                    std::fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
                    write(&destination.join("nodes.geojson"), &pretty(&exported.nodes))?;
                    write(&destination.join("edges.geojson"), &pretty(&exported.edges))
                }
                "inp" => {
                    let options = parse_options(&options)?;
                    let crs = options
                        .get("crs")
                        .ok_or("writing a .inp needs the coordinate reference system it was imported from; pass -o crs=EPSG:xxxx")?;
                    write(&destination, &trama_epanet::exporter::export_inp(&container, crs)?)
                }
                other => Err(format!("unsupported export format '{other}'")),
            }
        }
    }
}

fn load(source: &Path, options: &BTreeMap<String, String>) -> Result<Import, String> {
    let suffix = source
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    if source.is_dir() || NATIVE_SUFFIXES.contains(&suffix.as_str()) {
        return Ok(Import { features: features(source)?, extras: Vec::new(), channels: Vec::new() });
    }
    importers()
        .into_iter()
        .find(|importer| importer.suffixes().contains(&suffix.as_str()))
        .ok_or_else(|| format!("no installed importer claims '{suffix}'; install the package that reads it"))?
        .load(source, options)
}

/// A FeatureCollection, or a directory holding the two an export wrote.
fn features(source: &Path) -> Result<Vec<Value>, String> {
    let paths: Vec<PathBuf> = if source.is_dir() {
        let present: Vec<PathBuf> = ["edges.geojson", "nodes.geojson"]
            .iter()
            .map(|name| source.join(name))
            .filter(|path| path.exists())
            .collect();
        if present.is_empty() {
            return Err(format!("{} holds no edges.geojson or nodes.geojson", source.display()));
        }
        present
    } else {
        vec![source.to_path_buf()]
    };
    let mut features = Vec::new();
    for path in paths {
        let parsed: Value = serde_json::from_str(&read(&path)?).map_err(|error| error.to_string())?;
        features.extend(parsed["features"].as_array().cloned().unwrap_or_default());
    }
    Ok(features)
}

fn pretty(value: &Value) -> String {
    format!("{}\n", serde_json::to_string_pretty(value).unwrap())
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn bytes(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|error| format!("{}: {error}", path.display()))
}
