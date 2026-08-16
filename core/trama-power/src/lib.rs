// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reads an electrical network out of a pandapower JSON.
//!
//! This crate is where electrical knowledge lives. It knows that a bus is a point in a network
//! and a line is a span between two of them, that a transformer joins two voltage levels and
//! carries no geometry of its own, and that pandapower addresses every one of them by a pandas
//! index the rest of its tables reference. The core learns none of that: it receives GeoJSON
//! features carrying `power:` properties, and one opaque record holding what it cannot type.
//!
//! What travels in `XTRA` is the rest of the network — loads, generators, the external grid,
//! switches, standard types — plus the *schema* of the three tables this importer expresses.
//! The schema is not data another section could carry (SPEC 330): it is the column order a
//! reader needs to put the rows back, and the rows themselves are in `GRPH` and `PROP`.

pub mod flow;
pub mod network;
pub mod solver;

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Map, Value, json};
use trama_format::{Extra, Import, Importer};

pub const OWNER: &str = "power";
pub const MEDIA_TYPE: &str = "application/vnd.pandapower.network+json";
/// The three tables this importer turns into entities. Everything else travels whole.
const EXPRESSED: [&str; 3] = ["bus", "line", "trafo"];
/// How far apart two buses drawn at the same coordinate are placed, in metres.
///
/// A substation's high and low sides sit at one point on any map, and the transformer between
/// them is drawn as that point too. SPEC 4.2 identifies a node by where it is, so two buses at
/// one coordinate become one node — which shorts the transformer out and collapses a two-voltage
/// network into a single level. Ten metres is below what any network-scale view resolves and far
/// above the quantization floor, so the picture is unchanged and the graph is right.
const SEPARATION_METRES: f64 = 10.0;
/// One degree of longitude at the equator, in metres: 2πR/360 for the WGS 84 semi-major axis.
const METRES_PER_DEGREE: f64 = 111_319.49;

pub struct PowerImporter;

impl Importer for PowerImporter {
    fn id(&self) -> &'static str {
        "power"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        // `.json` already means "compile this as it stands", so this importer is reached by
        // name. Declared empty rather than claiming a suffix it would have to share.
        &[]
    }

    fn load(&self, source: &Path, _options: &BTreeMap<String, String>) -> Result<Import, String> {
        let text = std::fs::read_to_string(source).map_err(|error| format!("{}: {error}", source.display()))?;
        import(&text)
    }
}

/// What a container built from an electrical network may be solved for.
///
/// Neither declares a range. A bus below 0.9 p.u. and a line over 100% are the two answers an
/// operator runs the study to find, and a declared range would make the host reject exactly
/// those deltas as invalid — the failure state is not an invalid state.
pub fn channels() -> Vec<Value> {
    vec![
        json!({"name": "voltage", "entity_kind": "node", "unit": "p.u."}),
        json!({"name": "loading", "entity_kind": "edge", "unit": "%"}),
    ]
}

/// Turns a `pandapower.to_json` document into features the compiler already speaks.
pub fn import(text: &str) -> Result<Import, String> {
    let document: Value = serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let tables = document
        .get("_object")
        .and_then(Value::as_object)
        .ok_or("a pandapower JSON has an '_object' map of tables; write it with pandapower.to_json")?;

    let buses = frame(tables, "bus")?;
    let mut positions: BTreeMap<i64, (f64, f64)> = BTreeMap::new();
    let mut taken: BTreeMap<(u64, u64), u32> = BTreeMap::new();
    let mut features: Vec<Value> = Vec::new();
    for (index, row) in buses.rows() {
        let drawn = point(row.get("geo"))
            .ok_or_else(|| format!("bus {index} carries no point geometry, so it cannot be placed on a map"))?;
        let occupants = taken.entry((drawn.0.to_bits(), drawn.1.to_bits())).or_default();
        let position = separate(drawn, *occupants);
        *occupants += 1;
        positions.insert(index, position);
        let mut properties = named(&row);
        properties.insert("power:kind".into(), json!("bus"));
        properties.insert("power:index".into(), json!(index));
        features.push(json!({
            "type": "Feature",
            "properties": Value::Object(properties),
            "geometry": {"type": "Point", "coordinates": [position.0, position.1]},
        }));
    }

    // A line's own geometry carries its endpoints as well as its bends, and those endpoints are
    // whatever the source drew rather than exactly where the bus sits. The path is rebuilt from
    // the bus outward — bus, the row's interior vertices, bus — because SPEC 4.2 joins the graph
    // where two lines share an endpoint, and "nearly the same point" is not shared.
    for (table, from, to) in [("line", "from_bus", "to_bus"), ("trafo", "hv_bus", "lv_bus")] {
        for (index, row) in frame(tables, table)?.rows() {
            let ends = [terminal(&row, from, &positions, table, index)?, terminal(&row, to, &positions, table, index)?];
            let mut path = vec![json!([ends[0].0, ends[0].1])];
            path.extend(interior(row.get("geo")));
            path.push(json!([ends[1].0, ends[1].1]));
            let mut properties = named(&row);
            properties.insert("power:kind".into(), json!(table));
            properties.insert("power:index".into(), json!(index));
            features.push(json!({
                "type": "Feature",
                "properties": Value::Object(properties),
                "geometry": {"type": "LineString", "coordinates": path},
            }));
        }
    }

    Ok(Import {
        features,
        extras: vec![Extra {
            owner: OWNER.into(),
            media_type: MEDIA_TYPE.into(),
            payload: remainder(&document, tables)?.into_bytes(),
        }],
        channels: channels(),
    })
}

/// The document a reader needs to rebuild the network, minus the rows this importer expressed.
///
/// Each expressed table keeps its columns and loses its data: the columns are the order a
/// reader puts `PROP` values back in, and the rows are in `GRPH` and `PROP` already.
fn remainder(document: &Value, tables: &Map<String, Value>) -> Result<String, String> {
    let mut trimmed = document.clone();
    for name in EXPRESSED {
        let frame = frame(tables, name)?;
        let emptied = json!({"columns": frame.columns, "index": [], "data": []});
        trimmed["_object"][name]["_object"] = json!(serde_json::to_string(&emptied).map_err(|e| e.to_string())?);
    }
    serde_json::to_string(&trimmed).map_err(|error| error.to_string())
}

/// One pandas DataFrame as pandapower serializes it: a JSON string inside the JSON document.
struct Frame {
    columns: Vec<String>,
    index: Vec<i64>,
    data: Vec<Vec<Value>>,
}

impl Frame {
    /// Each row keyed by column name, in index order.
    fn rows(&self) -> Vec<(i64, BTreeMap<&str, &Value>)> {
        self.index
            .iter()
            .zip(&self.data)
            .map(|(index, values)| {
                (*index, self.columns.iter().map(String::as_str).zip(values).collect::<BTreeMap<&str, &Value>>())
            })
            .collect()
    }
}

fn frame(tables: &Map<String, Value>, name: &str) -> Result<Frame, String> {
    let encoded = tables
        .get(name)
        .and_then(|table| table.get("_object"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("this pandapower network has no '{name}' table"))?;
    let parsed: Value =
        serde_json::from_str(encoded).map_err(|error| format!("the '{name}' table is not JSON: {error}"))?;
    let strings = |key: &str| -> Vec<String> {
        parsed[key]
            .as_array()
            .map(|list| list.iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect())
            .unwrap_or_default()
    };
    let index: Vec<i64> =
        parsed["index"].as_array().map(|list| list.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    let data: Vec<Vec<Value>> = parsed["data"]
        .as_array()
        .map(|rows| rows.iter().map(|row| row.as_array().cloned().unwrap_or_default()).collect())
        .unwrap_or_default();
    if index.len() != data.len() {
        return Err(format!("the '{name}' table has {} rows against {} indices", data.len(), index.len()));
    }
    Ok(Frame { columns: strings("columns"), index, data })
}

/// Every column that carries a value, under its `power:` key. `geo` is left out: it became the
/// feature's geometry, and SPEC 330 will not have it stored twice.
fn named(row: &BTreeMap<&str, &Value>) -> Map<String, Value> {
    row.iter()
        .filter(|(column, value)| **column != "geo" && !value.is_null())
        .map(|(column, value)| (format!("power:{column}"), (*value).clone()))
        .collect()
}

fn terminal(
    row: &BTreeMap<&str, &Value>,
    column: &str,
    positions: &BTreeMap<i64, (f64, f64)>,
    table: &str,
    index: i64,
) -> Result<(f64, f64), String> {
    let bus =
        row.get(column).and_then(|value| value.as_i64()).ok_or_else(|| format!("{table} {index} names no {column}"))?;
    positions
        .get(&bus)
        .copied()
        .ok_or_else(|| format!("{table} {index} connects to bus {bus}, which this network does not have"))
}

/// The `nth` bus drawn at one coordinate, stepped east so each keeps its own node.
///
/// The first one stays exactly where the source put it, so a network without coincident buses
/// is untouched and the common case compiles to the same bytes it always did.
fn separate(drawn: (f64, f64), nth: u32) -> (f64, f64) {
    if nth == 0 {
        return drawn;
    }
    // Longitude degrees shrink with latitude, so the step is converted where the bus actually is.
    let eastward = SEPARATION_METRES / (METRES_PER_DEGREE * drawn.1.to_radians().cos());
    (drawn.0 + f64::from(nth) * eastward, drawn.1)
}

/// pandapower stores a row's geometry as a GeoJSON string in its own column.
fn geometry(value: Option<&&Value>) -> Option<Value> {
    serde_json::from_str(value?.as_str()?).ok()
}

fn point(value: Option<&&Value>) -> Option<(f64, f64)> {
    let coordinates = geometry(value)?;
    let pair = coordinates.get("coordinates")?.as_array()?;
    Some((pair.first()?.as_f64()?, pair.get(1)?.as_f64()?))
}

/// A line's bends, without the endpoints its buses already supply.
fn interior(value: Option<&&Value>) -> Vec<Value> {
    let Some(shape) = geometry(value) else { return Vec::new() };
    let Some(vertices) = shape.get("coordinates").and_then(Value::as_array) else { return Vec::new() };
    if vertices.len() < 3 {
        return Vec::new();
    }
    vertices[1..vertices.len() - 1].to_vec()
}
