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

/// The channels a street network can be solved for. SPEC declares channels; it never holds state.
///
/// Three of the four are topological and say nothing about roads: what a point reaches, what a
/// closure cuts off, which streets are the only way through. A container declares them because
/// they are true of any connected network, and because a solver may only write where the file
/// says it may — an undeclared channel is a calculation the file has refused.
pub fn channels() -> Vec<Value> {
    let mut declared: Vec<Value> = ["on_route", "reach", "isolated", "critical"]
        .iter()
        .map(|name| json!({"name": name, "entity_kind": "edge", "unit": "1", "min": 0, "max": 1}))
        .collect();
    // Which vehicle of a fleet serves a street, numbered from one, and zero where none does.
    // No declared range, unlike the four above: how many vehicles there are is the caller's
    // question, and a file that guessed at a maximum would have the host reject the fifth van
    // as an invalid reading.
    declared.push(json!({"name": "vehicle", "entity_kind": "edge", "unit": "1"}));
    declared
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
    // OSM node id -> the pieces that end on it, each with the way it came from.
    let mut touching: BTreeMap<u64, Vec<(u64, String)>> = BTreeMap::new();
    for element in elements {
        if element["type"] != "way" {
            continue;
        }
        let Some(way_id) = element["id"].as_u64() else {
            continue;
        };
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

        let spans = split_at_junctions(&coordinates, &nodes, &appearances);
        // Which piece of this way touches which OSM node, so a turn restriction naming two ways
        // and the node between them can find the two pieces that actually meet there. Recorded
        // while splitting because afterwards the node ids are gone: the compiler joins on
        // position, and a piece keeps no memory of what OSM called its ends.
        let boundaries = piece_boundaries(&spans, &coordinates, &nodes);
        for (piece, span) in spans.iter().enumerate() {
            let name = format!("osm:way/{}/{piece}", element["id"]);
            for node in boundaries.get(piece).into_iter().flatten() {
                touching.entry(*node).or_default().push((way_id, name.clone()));
            }
            features.push(json!({
                "type": "Feature",
                // Stable across recompilations because OSM's own way id is, and the piece index
                // is stable for as long as the way's node list is.
                "id": name,
                "properties": Value::Object(properties.clone()),
                "geometry": {"type": "LineString", "coordinates": span.clone()},
            }));
        }
    }

    if features.is_empty() {
        return Err("the extract holds no way with geometry; query with `out geom;`".into());
    }
    apply_restrictions(elements, &touching, &mut features);
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

/// The OSM node at each end of each piece, in the order [`split_at_junctions`] produced them.
///
/// A piece is a slice of the way's coordinates, so finding it in the original is what recovers
/// the node ids at its ends. Positions are compared rather than indices because the split returns
/// slices and not their bounds — and a way whose node list is out of step with its geometry was
/// never split, so it has one piece whose ends are the way's own.
fn piece_boundaries(spans: &[Vec<Value>], coordinates: &[Value], nodes: &[u64]) -> Vec<Vec<u64>> {
    if nodes.len() != coordinates.len() {
        return match (nodes.first(), nodes.last()) {
            (Some(first), Some(last)) => vec![vec![*first, *last]],
            _ => vec![Vec::new()],
        };
    }
    let mut ends = Vec::with_capacity(spans.len());
    let mut start = 0;
    for span in spans {
        let stop = start + span.len() - 1;
        ends.push(vec![nodes[start], nodes[stop.min(nodes.len() - 1)]]);
        start = stop;
    }
    ends
}

/// Writes each turn restriction onto the edge a driver would be arriving on.
///
/// An OSM restriction names two ways and the node between them: come in along `from`, and at
/// `via` you may not take `to`. The pieces are what the graph actually holds, so the relation is
/// resolved to the one piece of each way that touches `via` — the rest of either way is somewhere
/// else entirely and unaffected.
///
/// `only_*` is the same statement inverted: taking anything but `to` is what is forbidden, so it
/// expands to a prohibition against every other piece at that node. Storing it expanded rather
/// than as a mode keeps the reader trivial — a router asks one question, "may I go from here to
/// there", and never has to know which spelling produced the answer.
///
/// ponytail: `via` must be a node. A restriction whose `via` is a way — a no-U-turn across a dual
/// carriageway, mostly — spans two junctions and cannot be expressed as a property of one edge;
/// those are skipped, and a router will happily route through them. Expressing them needs the
/// path-shaped state a property on an edge does not have.
fn apply_restrictions(elements: &[Value], touching: &BTreeMap<u64, Vec<(u64, String)>>, features: &mut [Value]) {
    let mut forbidden: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for element in elements {
        if element["type"] != "relation" || element["tags"]["type"] != "restriction" {
            continue;
        }
        let Some(kind) = element["tags"]["restriction"].as_str() else {
            continue;
        };
        let only = match kind.split('_').next() {
            Some("no") => false,
            Some("only") => true,
            // `restriction=give_way` and friends describe priority, not permission.
            _ => continue,
        };
        let member = |role: &str, kind: &str| {
            element["members"].as_array()?.iter().find(|member| member["role"] == role && member["type"] == kind)?
                ["ref"]
                .as_u64()
        };
        // A `via` way is a restriction over a path rather than a turn; see the note above.
        let (Some(from), Some(via), Some(to)) = (member("from", "way"), member("via", "node"), member("to", "way"))
        else {
            continue;
        };
        let Some(pieces) = touching.get(&via) else {
            continue;
        };
        let arriving: Vec<&String> = pieces.iter().filter(|(way, _)| *way == from).map(|(_, name)| name).collect();
        let leaving: Vec<&String> = pieces
            .iter()
            .filter(|(way, _)| if only { *way != from && *way != to } else { *way == to })
            .map(|(_, name)| name)
            // An `only_*` restriction names the one movement that is allowed, and what it forbids
            // is every *other* exit. Turning back the way you came is not an exit anyone chose to
            // name, and OSM spells that prohibition `no_u_turn` — so it is left to that spelling
            // rather than smuggled in here. A `no_*` keeps its own pieces: `no_u_turn` names the
            // same way as `from` and `to`, and dropping them would leave it forbidding nothing,
            // which is what it did until a real extract turned up 25 of them doing exactly that.
            .filter(|name| !only || !arriving.contains(name))
            .collect();
        for entry in arriving {
            for exit in &leaving {
                forbidden.entry(entry.clone()).or_default().push((*exit).clone());
            }
        }
    }

    for feature in features.iter_mut() {
        let Some(names) = feature["id"].as_str().and_then(|id| forbidden.get(id)) else {
            continue;
        };
        // The stable id of each forbidden edge, space-separated. Identity rather than text: an
        // edge's id is derived from this same declared string, so the importer can name an edge
        // the file does not hold yet, and a router reads ids without a lookup table. The rule is
        // imported from `trama-format` rather than copied — a copy would be a bug the moment the
        // format changed how it derives one.
        let mut ids: Vec<String> = names.iter().map(|name| trama_format::edge_id(name).to_string()).collect();
        ids.sort();
        ids.dedup();
        feature["properties"][RESTRICTION_KEY] = json!(ids.join(" "));
    }
}

/// The column a router reads to know which turns are forbidden. Domain knowledge, like
/// `roads:speed_ms`: the router is told the name and never learns what a turn is.
pub const RESTRICTION_KEY: &str = "roads:no_turn";

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
        // A slip road is not the road it joins: it is taken slower, and giving it the parent's
        // speed makes every interchange look like a shortcut. Cheaper than the road it leaves and
        // dearer than the street it lands on, which is what driving one feels like.
        Some("motorway_link") => 60.0,
        Some("trunk_link") => 50.0,
        Some("primary_link") | Some("secondary_link") | Some("tertiary_link") => 40.0,
        Some("motorway") => 120.0,
        Some("trunk") => 90.0,
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
