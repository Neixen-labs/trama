// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reads a road network out of an OpenStreetMap extract.
//!
//! This crate is where road knowledge lives. It knows that `oneway` names a traversal
//! restriction, that OSM spells it four ways, and that one of those spellings means the street
//! runs against the direction its own geometry is stored in. The core learns none of that: it
//! receives GeoJSON features carrying the reserved `_trama_directed` key and nothing else.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};
use trama_format::{Import, Importer};

pub struct RoadImporter;

impl Importer for RoadImporter {
    fn id(&self) -> &'static str {
        "roads"
    }

    fn suffixes(&self) -> &'static [&'static str] {
        // Overpass writes `.json`, which the compiler already claims, so this importer is
        // reached by name rather than by suffix. Declared anyway for a file named plainly.
        &[".osm.json"]
    }

    fn load(&self, source: &Path, _options: &BTreeMap<String, String>) -> Result<Import, String> {
        let text = std::fs::read_to_string(source).map_err(|error| format!("{}: {error}", source.display()))?;
        import(&text)
    }
}

/// The channel a routed network can be solved for. SPEC declares channels; it never holds state.
pub fn channels() -> Vec<Value> {
    vec![json!({"name": "on_route", "entity_kind": "edge", "unit": "1", "min": 0, "max": 1})]
}

/// Turns an Overpass `out geom` response into features the compiler already speaks.
pub fn import(text: &str) -> Result<Import, String> {
    let document: Value = serde_json::from_str(text).map_err(|error| format!("not JSON: {error}"))?;
    let elements = document["elements"]
        .as_array()
        .ok_or("an Overpass response has an 'elements' array; export with `out geom;`")?;

    // A junction is a node two ways share, and in OSM it is usually mid-way through at least one
    // of them. TRAMA takes a node from each end of a LineString and nowhere else (SPEC 4.2), so a
    // way handed over whole crosses its neighbours without ever meeting them: the map draws
    // perfectly and the graph is dust. Counting node references first is what finds the junctions.
    let mut appearances: BTreeMap<u64, u32> = BTreeMap::new();
    for element in elements {
        if element["type"] != "way" || !element["geometry"].is_array() {
            continue;
        }
        for node in element["nodes"].as_array().into_iter().flatten() {
            if let Some(id) = node.as_u64() {
                *appearances.entry(id).or_default() += 1;
            }
        }
    }

    let mut features = Vec::new();
    for element in elements {
        if element["type"] != "way" {
            continue;
        }
        let Some(geometry) = element["geometry"].as_array() else {
            // `out;` without `geom` returns node references rather than positions. Nothing here
            // can resolve those, and a way with no shape is not a road we can place.
            continue;
        };
        let mut coordinates: Vec<Value> =
            geometry.iter().filter_map(|point| Some(json!([point["lon"].as_f64()?, point["lat"].as_f64()?]))).collect();
        if coordinates.len() < 2 {
            continue;
        }
        let mut nodes: Vec<u64> = element["nodes"].as_array().into_iter().flatten().filter_map(Value::as_u64).collect();
        let tags = element["tags"].as_object();
        let oneway = tags.and_then(|tags| tags.get("oneway")).and_then(Value::as_str).unwrap_or("no");
        // SPEC 9 gives an edge no way to carry a sign: direction is the stored vertex order, so
        // a street tagged as running backwards is stored backwards. Reversing before splitting
        // leaves every piece pointing the way traffic goes.
        if oneway == "-1" {
            coordinates.reverse();
            nodes.reverse();
        }
        let mut properties = serde_json::Map::new();
        for (key, value) in tags.into_iter().flatten() {
            properties.insert(format!("osm:{key}"), text_of(value));
        }
        if directed(oneway) {
            properties.insert("_trama_directed".into(), Value::Bool(true));
        }
        // A speed a solver can use without knowing what `maxspeed` or `mph` are. Derived rather
        // than copied, so it carries the same unit for every way whatever the source said.
        let tag = |key: &str| tags.and_then(|tags| tags.get(key)).and_then(Value::as_str);
        properties.insert("roads:speed_ms".into(), json!(speed_metres_per_second(tag("maxspeed"), tag("highway"))));

        for (piece, span) in split_at_junctions(&coordinates, &nodes, &appearances).iter().enumerate() {
            features.push(json!({
                "type": "Feature",
                // Stable across recompilations because OSM's own way id is, and the piece index
                // is stable for as long as the way's node list is.
                "id": format!("osm:way/{}/{piece}", element["id"]),
                "properties": Value::Object(properties.clone()),
                "geometry": {"type": "LineString", "coordinates": span.clone()},
            }));
        }
    }

    if features.is_empty() {
        return Err("the extract holds no way with geometry; query with `out geom;`".into());
    }
    Ok(Import { features, extras: Vec::new(), channels: channels() })
}

/// Cuts a way wherever it touches a node another way also touches, so crossings become junctions.
///
/// Each piece keeps the shared vertex at both ends, which is what lets the compiler join them:
/// two pieces meeting at a junction each carry that position as an endpoint, and SPEC 4.2 makes
/// endpoints on the same quantization cell one node.
///
/// A way whose node list is missing or out of step with its geometry is not split. That happens
/// when an extract is post-processed, and one long edge is a worse graph but still a true one.
fn split_at_junctions(coordinates: &[Value], nodes: &[u64], appearances: &BTreeMap<u64, u32>) -> Vec<Vec<Value>> {
    if nodes.len() != coordinates.len() {
        return vec![coordinates.to_vec()];
    }
    let mut pieces = Vec::new();
    let mut start = 0;
    for (index, node) in nodes.iter().enumerate() {
        let shared = appearances.get(node).copied().unwrap_or(0) > 1;
        if index > start && (shared || index == nodes.len() - 1) {
            pieces.push(coordinates[start..=index].to_vec());
            start = index;
        }
    }
    if pieces.is_empty() {
        pieces.push(coordinates.to_vec());
    }
    pieces
}

/// A travelling speed in metres per second, from the tag if there is one and the road class if not.
///
/// Only 43% of the ways in the sample extract carry `maxspeed`, and every one that does not is a
/// `residential`, `living_street` or `tertiary` street — so without a fallback most of a city
/// would have no cost at all.
///
/// ponytail: the fallbacks are urban Spain, where `residential` has been 30 km/h since 2021.
/// They are wrong for a motorway network and wrong for most other countries. A real one would
/// come from the extract's country, which an extract does not state.
fn speed_metres_per_second(maxspeed: Option<&str>, highway: Option<&str>) -> f64 {
    let declared = maxspeed.and_then(|value| {
        let value = value.trim();
        match value {
            // OSM's named speeds. `none` is a derestricted motorway, not an absent limit.
            "walk" => Some(7.0),
            "none" => Some(130.0),
            _ => match value.strip_suffix(" mph") {
                Some(number) => number.trim().parse::<f64>().ok().map(|mph| mph * 1.609344),
                None => value.parse::<f64>().ok(),
            },
        }
    });
    let kilometres_per_hour = declared.filter(|speed| *speed > 0.0).unwrap_or(match highway {
        Some("living_street") => 20.0,
        Some("residential") | Some("unclassified") => 30.0,
        Some("primary") | Some("secondary") | Some("tertiary") => 50.0,
        _ => 30.0,
    });
    kilometres_per_hour / 3.6
}

/// The four spellings OSM uses for a one-way street.
///
/// `-1` is directed too: it means the street runs against its own geometry, which the caller has
/// already reversed. Every other value, including `no` and `reversible`, is two-way here — a
/// reversible street changes direction on a schedule this format has nowhere to put.
fn directed(oneway: &str) -> bool {
    matches!(oneway, "yes" | "true" | "1" | "-1")
}

/// OSM tag values are strings; a number in the source stays readable rather than becoming one.
fn text_of(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.clone()),
        other => Value::String(other.to_string()),
    }
}
