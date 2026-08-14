// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reads a SWMM `.inp` into the features and opaque records the compiler accepts.
//!
//! The node and link sections become entities with typed properties; `[XSECTIONS]` folds into
//! the link it names, because a cross-section is not an entity — it is the shape of one.
//! Sections whose row layout varies by type (outfalls, storage, weirs…) keep their tail as one
//! `swmm:parameters` string, the same treatment EPANET's pumps get: SWMM reads back what it
//! wrote, and a column whose meaning depends on another column is not a column.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use trama_epanet::importer::Reprojection;
use trama_epanet::inp;
use trama_format::{Extra, Import, Importer};

pub const OWNER: &str = "swmm";
pub const MEDIA_TYPE: &str = "text/vnd.swmm.inp-sections";
pub const EXPRESSED: [&str; 12] = [
    "JUNCTIONS",
    "OUTFALLS",
    "STORAGE",
    "DIVIDERS",
    "CONDUITS",
    "PUMPS",
    "ORIFICES",
    "WEIRS",
    "OUTLETS",
    "XSECTIONS",
    "COORDINATES",
    "VERTICES",
];

/// `[OPTIONS] FLOW_UNITS` sets the file's unit system; SWMM reports depth in feet for the US
/// units and in metres for the SI ones.
pub const US_FLOW_UNITS: [&str; 3] = ["cfs", "gpm", "mgd"];
pub const SI_FLOW_UNITS: [&str; 3] = ["cms", "lps", "mld"];
const DEFAULT_FLOW_UNITS: &str = "cfs";

/// Typed fields per node section, after the name. `#` parses as a number; a `*` entry keeps
/// the whole remaining tail as one `swmm:parameters` string.
const NODE_FIELDS: [(&str, &[&str]); 4] = [
    ("JUNCTIONS", &["#invert", "#max-depth", "#init-depth", "#surcharge-depth", "#ponded-area"]),
    ("OUTFALLS", &["#invert", "*"]),
    ("STORAGE", &["#invert", "#max-depth", "#init-depth", "*"]),
    ("DIVIDERS", &["#invert", "diverted-link", "*"]),
];
/// Typed fields per link section, after name and endpoints.
const LINK_FIELDS: [(&str, &[&str]); 5] = [
    ("CONDUITS", &["#length", "#roughness", "#in-offset", "#out-offset", "#init-flow", "#max-flow"]),
    ("PUMPS", &["*"]),
    ("ORIFICES", &["orifice-type", "#offset", "#coeff", "gated", "#close-time"]),
    ("WEIRS", &["weir-type", "*"]),
    ("OUTLETS", &["#offset", "*"]),
];
/// `[XSECTIONS]` fields after the link name, folded into that link's properties.
const XSECTION_FIELDS: [&str; 7] = ["shape", "#geom1", "#geom2", "#geom3", "#geom4", "#barrels", "culvert"];

pub struct SwmmImporter;

/// Import from text rather than a path, which is what a browser has.
pub fn import(text: &str, crs: &str) -> Result<Import, String> {
    SwmmImporter.read(text, crs)
}

impl Importer for SwmmImporter {
    fn id(&self) -> &'static str {
        "swmm"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        // `.inp` belongs to the EPANET importer, which was here first; this one is reached by
        // `--importer swmm`. Each importer tells the caller about the other when handed the
        // wrong file, so the collision costs one clear message rather than a wrong parse.
        &[]
    }

    fn load(&self, source: &Path, options: &BTreeMap<String, String>) -> Result<Import, String> {
        let crs = options
            .get("source-crs")
            .filter(|value| !value.is_empty())
            .ok_or("a SWMM .inp declares no coordinate reference system; pass -o source-crs=EPSG:xxxx")?;
        let text = std::fs::read_to_string(source).map_err(|error| error.to_string())?;
        self.read(&text, crs)
    }
}

impl SwmmImporter {
    fn read(&self, text: &str, crs: &str) -> Result<Import, String> {
        let document = inp::parse(text);
        if !document.rows("PIPES").is_empty() || !document.rows("RESERVOIRS").is_empty() {
            return Err(
                "this looks like an EPANET network ([PIPES]/[RESERVOIRS]); pass --importer epanet or drop --importer"
                    .into(),
            );
        }
        let transform = Reprojection::to_wgs84(crs)?;

        let mut positions: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for row in document.rows("COORDINATES") {
            positions.insert(row[0].clone(), (number(&row[1])?, number(&row[2])?));
        }
        let mut vertices: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
        for row in document.rows("VERTICES") {
            vertices.entry(row[0].clone()).or_default().push((number(&row[1])?, number(&row[2])?));
        }
        let mut xsections: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
        for row in document.rows("XSECTIONS") {
            xsections.insert(row[0].clone(), named(&XSECTION_FIELDS, &row[1..])?);
        }

        let mut features: Vec<Value> = Vec::new();
        for (section, fields) in NODE_FIELDS {
            for row in document.rows(section) {
                let mut properties = named(fields, &row[1..])?;
                properties.insert("swmm:kind".into(), json!(kind_of(section)));
                properties.insert("swmm:name".into(), json!(row[0]));
                let position = transform.apply(*place(&row[0], &positions)?)?;
                features.push(json!({
                    "type": "Feature",
                    "properties": Value::Object(properties),
                    "geometry": {"type": "Point", "coordinates": [position.0, position.1]},
                }));
            }
        }
        for (section, fields) in LINK_FIELDS {
            for row in document.rows(section) {
                let mut properties = named(fields, &row[3..])?;
                properties.insert("swmm:kind".into(), json!(kind_of(section)));
                properties.insert("swmm:name".into(), json!(row[0]));
                properties.extend(xsections.get(&row[0]).cloned().unwrap_or_default());
                let mut path = vec![*place(&row[1], &positions)?];
                path.extend(vertices.get(&row[0]).cloned().unwrap_or_default());
                path.push(*place(&row[2], &positions)?);
                let coordinates: Result<Vec<Value>, String> =
                    path.into_iter().map(|point| transform.apply(point).map(|(x, y)| json!([x, y]))).collect();
                features.push(json!({
                    "type": "Feature",
                    "id": row[0],
                    "properties": Value::Object(properties),
                    "geometry": {"type": "LineString", "coordinates": coordinates?},
                }));
            }
        }

        let remainder = inp::serialize(&document.without(&EXPRESSED));
        Ok(Import {
            features,
            extras: vec![Extra { owner: OWNER.into(), media_type: MEDIA_TYPE.into(), payload: remainder.into_bytes() }],
            channels: channels(&document)?,
        })
    }
}

/// What a container built from this file may be solved for, in the file's own units.
pub fn channels(document: &inp::Document) -> Result<Vec<Value>, String> {
    let flow_units = document
        .rows("OPTIONS")
        .into_iter()
        .find(|row| row[0].eq_ignore_ascii_case("flow_units"))
        .and_then(|row| row.last().map(|value| value.to_lowercase()))
        .unwrap_or_else(|| DEFAULT_FLOW_UNITS.to_string());
    if !US_FLOW_UNITS.contains(&flow_units.as_str()) && !SI_FLOW_UNITS.contains(&flow_units.as_str()) {
        return Err(format!("[OPTIONS] FLOW_UNITS names '{flow_units}', which is not a SWMM flow unit"));
    }
    let depth = if US_FLOW_UNITS.contains(&flow_units.as_str()) { "ft" } else { "m" };
    let mut channels = vec![
        json!({"name": "depth", "entity_kind": "node", "unit": depth}),
        json!({"name": "flow", "entity_kind": "edge", "unit": flow_units}),
        // The rate at which a node's inflow exceeds what it can pass or store: where the
        // system floods, which is the question stormwater exists to answer. Zero almost
        // everywhere almost always, which is exactly why the exceptions matter.
        json!({"name": "flooding", "entity_kind": "node", "unit": flow_units}),
    ];
    // The topological channels: close this conduit, what stops draining. Same declaration, and
    // the same solver, as a pipe network or a street network.
    channels.extend(
        ["reach", "isolated", "critical"]
            .iter()
            .map(|name| json!({"name": name, "entity_kind": "edge", "unit": "1", "min": 0, "max": 1})),
    );
    Ok(channels)
}

pub fn kind_of(section: &str) -> &'static str {
    match section {
        "JUNCTIONS" => "junction",
        "OUTFALLS" => "outfall",
        "STORAGE" => "storage",
        "DIVIDERS" => "divider",
        "CONDUITS" => "conduit",
        "PUMPS" => "pump",
        "ORIFICES" => "orifice",
        "WEIRS" => "weir",
        _ => "outlet",
    }
}

/// Name the fields a row carries. `*` swallows the remaining tail as one string; a trailing
/// field SWMM omits stays absent.
fn named(names: &[&str], row: &[String]) -> Result<Map<String, Value>, String> {
    let mut described = Map::new();
    for (at, name) in names.iter().enumerate() {
        if *name == "*" {
            let tail = row.get(at..).unwrap_or_default().join(" ");
            if !tail.is_empty() {
                described.insert("swmm:parameters".into(), json!(tail));
            }
            break;
        }
        let Some(value) = row.get(at).filter(|value| !value.is_empty()) else { continue };
        let key = format!("swmm:{}", name.trim_start_matches('#'));
        described.insert(key, if name.starts_with('#') { json!(number(value)?) } else { json!(value) });
    }
    Ok(described)
}

fn place<'a>(name: &str, positions: &'a BTreeMap<String, (f64, f64)>) -> Result<&'a (f64, f64), String> {
    positions
        .get(name)
        .ok_or_else(|| format!("node '{name}' has no entry in [COORDINATES], so it cannot be placed on a map"))
}

fn number(value: &str) -> Result<f64, String> {
    value.parse::<f64>().map_err(|_| format!("'{value}' is not a number"))
}
