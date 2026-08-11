// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reads `.inp` into the features and opaque records the compiler accepts.
//!
//! The six sections that define entities become nodes, edges and typed properties;
//! `[COORDINATES]` and `[VERTICES]` become geometry; every other section travels verbatim in
//! one `XTRA` record, because a demand pattern is not a value attached to an entity and
//! SPEC 7.1 will not have it faked as one.

use std::collections::BTreeMap;
use std::path::Path;

use proj4rs::Proj;
use serde_json::{Map, Value, json};
use trama_format::{Extra, Import, Importer};

use crate::inp;

pub const OWNER: &str = "epanet";
pub const MEDIA_TYPE: &str = "text/vnd.epanet.inp-sections";
const EXPRESSED: [&str; 8] =
    ["JUNCTIONS", "RESERVOIRS", "TANKS", "PIPES", "PUMPS", "VALVES", "COORDINATES", "VERTICES"];

/// `[OPTIONS] Units` sets the whole file's unit system, and EPANET reports pressure in psi for
/// the US flow units and in metres for the SI ones. A channel declaration naming the wrong one
/// would be a lie the file tells every solver that reads it.
pub const US_FLOW_UNITS: [&str; 5] = ["cfs", "gpm", "mgd", "imgd", "afd"];
pub const SI_FLOW_UNITS: [&str; 5] = ["lps", "lpm", "mld", "cmh", "cmd"];
pub const PRESSURE_UNITS: [&str; 2] = ["psi", "m"];
const DEFAULT_FLOW_UNITS: &str = "gpm";

/// Field names per section, in file order. A `#` prefix stores the field as a number; anything
/// else stays a string, because EPANET allows a curve or pattern id in those columns and one
/// string beats a column whose type depends on the network.
const NODE_FIELDS: [(&str, &[&str]); 3] = [
    ("JUNCTIONS", &["#elevation", "#demand", "pattern"]),
    ("RESERVOIRS", &["#head", "pattern"]),
    (
        "TANKS",
        &[
            "#elevation",
            "#init-level",
            "#min-level",
            "#max-level",
            "#diameter",
            "#min-volume",
            "volume-curve",
            "overflow",
        ],
    ),
];
const LINK_FIELDS: [(&str, &[&str]); 2] = [
    ("PIPES", &["#length", "#diameter", "#roughness", "#minor-loss", "status"]),
    ("VALVES", &["#diameter", "valve-type", "setting", "#minor-loss"]),
];

pub struct EpanetImporter;

/// Import from text rather than a path, which is what a browser has.
pub fn import(text: &str, crs: &str) -> Result<Import, String> {
    EpanetImporter.read(text, crs)
}

impl Importer for EpanetImporter {
    fn id(&self) -> &'static str {
        "epanet"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        &[".inp"]
    }

    fn load(&self, source: &Path, options: &BTreeMap<String, String>) -> Result<Import, String> {
        let crs = options
            .get("source-crs")
            .filter(|value| !value.is_empty())
            .ok_or("an EPANET .inp declares no coordinate reference system; pass -o source-crs=EPSG:xxxx")?;
        let text = std::fs::read_to_string(source).map_err(|error| error.to_string())?;
        self.read(&text, crs)
    }
}

impl EpanetImporter {
    fn read(&self, text: &str, crs: &str) -> Result<Import, String> {
        let document = inp::parse(text);
        let transform = Reprojection::to_wgs84(crs)?;

        let mut positions: BTreeMap<String, (f64, f64)> = BTreeMap::new();
        for row in document.rows("COORDINATES") {
            positions.insert(row[0].clone(), (number(&row[1])?, number(&row[2])?));
        }
        let mut vertices: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
        for row in document.rows("VERTICES") {
            vertices.entry(row[0].clone()).or_default().push((number(&row[1])?, number(&row[2])?));
        }

        let mut features: Vec<Value> = Vec::new();
        for (section, fields) in NODE_FIELDS {
            for row in document.rows(section) {
                let mut properties = named(fields, &row[1..])?;
                properties.insert("epanet:kind".into(), json!(kind_of(section)));
                properties.insert("epanet:name".into(), json!(row[0]));
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
                properties.insert("epanet:kind".into(), json!(kind_of(section)));
                properties.insert("epanet:name".into(), json!(row[0]));
                features.push(link(&row, properties, &positions, &vertices, &transform)?);
            }
        }
        // A pump's parameters are keyword-value pairs whose count and meaning vary; the line is
        // kept whole rather than guessed at, and EPANET reads back what it wrote.
        for row in document.rows("PUMPS") {
            let mut properties = Map::new();
            properties.insert("epanet:kind".into(), json!("pump"));
            properties.insert("epanet:name".into(), json!(row[0]));
            properties.insert("epanet:parameters".into(), json!(row[3..].join(" ")));
            features.push(link(&row, properties, &positions, &vertices, &transform)?);
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
        .find(|row| row[0].eq_ignore_ascii_case("units"))
        .and_then(|row| row.last().map(|value| value.to_lowercase()))
        .unwrap_or_else(|| DEFAULT_FLOW_UNITS.to_string());
    // An unknown keyword would become a declared unit no solver could match, and the mismatch
    // would surface at solve time rather than here, where the file that caused it is in hand.
    if !US_FLOW_UNITS.contains(&flow_units.as_str()) && !SI_FLOW_UNITS.contains(&flow_units.as_str()) {
        return Err(format!("[OPTIONS] Units names '{flow_units}', which is not an EPANET flow unit"));
    }
    let pressure = if US_FLOW_UNITS.contains(&flow_units.as_str()) { "psi" } else { "m" };
    Ok(vec![
        json!({"name": "pressure", "entity_kind": "node", "unit": pressure}),
        json!({"name": "flow", "entity_kind": "edge", "unit": flow_units}),
    ])
}

fn kind_of(section: &str) -> &'static str {
    match section {
        "JUNCTIONS" => "junction",
        "RESERVOIRS" => "reservoir",
        "TANKS" => "tank",
        "PIPES" => "pipe",
        "VALVES" => "valve",
        _ => "pump",
    }
}

/// Name the fields a row actually carries. A trailing field EPANET omits stays absent.
fn named(names: &[&str], row: &[String]) -> Result<Map<String, Value>, String> {
    let mut described = Map::new();
    for (name, value) in names.iter().zip(row) {
        if value.is_empty() {
            continue;
        }
        let key = format!("epanet:{}", name.trim_start_matches('#'));
        described.insert(key, if name.starts_with('#') { json!(number(value)?) } else { json!(value) });
    }
    Ok(described)
}

fn link(
    row: &[String],
    properties: Map<String, Value>,
    positions: &BTreeMap<String, (f64, f64)>,
    vertices: &BTreeMap<String, Vec<(f64, f64)>>,
    transform: &Reprojection,
) -> Result<Value, String> {
    let mut path = vec![*place(&row[1], positions)?];
    path.extend(vertices.get(&row[0]).cloned().unwrap_or_default());
    path.push(*place(&row[2], positions)?);
    let coordinates: Result<Vec<Value>, String> =
        path.into_iter().map(|point| transform.apply(point).map(|(x, y)| json!([x, y]))).collect();
    Ok(json!({
        "type": "Feature",
        "id": row[0],
        "properties": Value::Object(properties),
        "geometry": {"type": "LineString", "coordinates": coordinates?},
    }))
}

fn place<'a>(name: &str, positions: &'a BTreeMap<String, (f64, f64)>) -> Result<&'a (f64, f64), String> {
    positions
        .get(name)
        .ok_or_else(|| format!("node '{name}' has no entry in [COORDINATES], so it cannot be placed on a map"))
}

fn number(value: &str) -> Result<f64, String> {
    value.parse::<f64>().map_err(|_| format!("'{value}' is not a number"))
}

/// Source coordinates to WGS 84. Pure Rust, so it works in a browser as well as a terminal.
pub struct Reprojection {
    source: Proj,
    target: Proj,
}

impl Reprojection {
    pub fn to_wgs84(crs: &str) -> Result<Self, String> {
        Ok(Self { source: definition(crs)?, target: definition("EPSG:4326")? })
    }

    pub fn from_wgs84(crs: &str) -> Result<Self, String> {
        Ok(Self { source: definition("EPSG:4326")?, target: definition(crs)? })
    }

    pub fn apply(&self, point: (f64, f64)) -> Result<(f64, f64), String> {
        let mut moved = if self.source.is_latlong() {
            (point.0.to_radians(), point.1.to_radians(), 0.0)
        } else {
            (point.0, point.1, 0.0)
        };
        proj4rs::transform::transform(&self.source, &self.target, &mut moved)
            .map_err(|error| format!("cannot reproject: {error}"))?;
        if self.target.is_latlong() { Ok((moved.0.to_degrees(), moved.1.to_degrees())) } else { Ok((moved.0, moved.1)) }
    }
}

/// `EPSG:25830` or a proj string. proj4rs speaks the second; the EPSG registry is a table.
fn definition(crs: &str) -> Result<Proj, String> {
    let text = match crs.strip_prefix("EPSG:").or_else(|| crs.strip_prefix("epsg:")) {
        Some(code) => {
            let code: u16 = code.parse().map_err(|_| format!("'{crs}' is not an EPSG code"))?;
            crs_definitions::from_code(code)
                .ok_or_else(|| format!("EPSG:{code} is not in the registry this build carries"))?
                .proj4
                .to_string()
        }
        None => crs.to_string(),
    };
    Proj::from_user_string(&text).map_err(|error| format!("unknown coordinate reference system '{crs}': {error}"))
}
