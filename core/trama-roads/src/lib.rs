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
    // Way id -> its pieces and the two OSM nodes each one runs between. A restriction whose `via`
    // is a way has to be walked across those pieces, which needs both ends and not just the fact
    // that a piece touches a node — `touching` cannot say which end of which piece leads where.
    let mut spans_of: BTreeMap<u64, Vec<Span>> = BTreeMap::new();
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
            if let Some([first, last]) = boundaries
                .get(piece)
                .map(Vec::as_slice)
                .and_then(|ends| ends.first().zip(ends.last()))
                .map(|(a, b)| [*a, *b])
            {
                spans_of.entry(way_id).or_default().push((name.clone(), first, last));
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
    apply_restrictions(elements, &touching, &spans_of, &mut features);
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
/// An OSM restriction names two ways and what lies between them: come in along `from`, and past
/// `via` you may not take `to`. The pieces are what the graph actually holds, so the relation is
/// resolved to the pieces of each way that actually meet — the rest of either way is somewhere
/// else entirely and unaffected.
///
/// `via` is usually a node, and then the restriction is a movement between two edges. Sometimes it
/// is one or more *ways*: a no-U-turn across a dual carriageway comes in on one carriageway,
/// crosses the link between them, and may not come back down the other. That is the same statement
/// with more edges in the middle, and it is written as the run of edges it forbids — which is why
/// the column holds runs rather than pairs, and why a pair is just the run of length two.
///
/// `only_*` is the same statement inverted: taking anything but `to` is what is forbidden, so it
/// expands to a prohibition against every other piece at the far end. Storing it expanded rather
/// than as a mode keeps the reader trivial — a router asks one question, "may I make this run of
/// movements", and never has to know which spelling produced the answer.
fn apply_restrictions(
    elements: &[Value],
    touching: &BTreeMap<u64, Vec<(u64, String)>>,
    spans_of: &BTreeMap<u64, Vec<Span>>,
    features: &mut [Value],
) {
    // Each entry edge's forbidden continuations, every one a run of pieces to be walked in order.
    let mut forbidden: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
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
        let members = |role: &str, kind: &str| -> Vec<u64> {
            element["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|member| member["role"] == role && member["type"] == kind)
                .filter_map(|member| member["ref"].as_u64())
                .collect()
        };
        let (from, to) = (members("from", "way"), members("to", "way"));
        let (Some(from), Some(to)) = (from.first().copied(), to.first().copied()) else {
            continue;
        };
        // A `via` node is the junction itself and needs no crossing; `via` ways are crossed, and
        // the junctions at their two ends are what `from` and `to` meet. Both end up as a run.
        let (entries, chain, exit_node) = match members("via", "node").first().copied() {
            Some(node) => (node, Vec::new(), node),
            None => {
                let pieces: Vec<Span> =
                    members("via", "way").iter().filter_map(|way| spans_of.get(way)).flatten().cloned().collect();
                let Some(crossing) = cross(&pieces, touching, from, to) else {
                    continue;
                };
                crossing
            }
        };
        let Some(at_entry) = touching.get(&entries) else {
            continue;
        };
        let Some(at_exit) = touching.get(&exit_node) else {
            continue;
        };
        let arriving: Vec<&String> = at_entry.iter().filter(|(way, _)| *way == from).map(|(_, name)| name).collect();
        let crossed: Vec<String> = chain.iter().map(|(name, _, _)| name.clone()).collect();
        let leaving: Vec<&String> = at_exit
            .iter()
            .filter(|(way, _)| if only { *way != from && *way != to } else { *way == to })
            .map(|(_, name)| name)
            // An `only_*` restriction names the one movement that is allowed, and what it forbids
            // is every *other* exit. Turning back the way you came is not an exit anyone chose to
            // name, and OSM spells that prohibition `no_u_turn` — so it is left to that spelling
            // rather than smuggled in here. A `no_*` keeps its own pieces: `no_u_turn` names the
            // same way as `from` and `to`, and dropping them would leave it forbidding nothing,
            // which is what it did until a real extract turned up 25 of them doing exactly that.
            .filter(|name| !only || (!arriving.contains(name) && !crossed.contains(name)))
            .collect();
        for entry in arriving {
            for exit in &leaving {
                let mut run = crossed.clone();
                run.push((*exit).clone());
                forbidden.entry(entry.clone()).or_default().push(run);
            }
        }
    }

    for feature in features.iter_mut() {
        let Some(runs) = feature["id"].as_str().and_then(|id| forbidden.get(id)) else {
            continue;
        };
        // Each run as the stable ids of the edges it crosses, joined by `>`, and the runs
        // separated by spaces. Identity rather than text: an edge's id is derived from this same
        // declared string, so the importer can name an edge the file does not hold yet, and a
        // router reads ids without a lookup table. The rule is imported from `trama-format`
        // rather than copied — a copy would be a bug the moment the format changed how it derives
        // one. A run of one id is the ordinary turn and is written exactly as it always was.
        let mut written: Vec<String> = runs
            .iter()
            .map(|run| run.iter().map(|name| trama_format::edge_id(name).to_string()).collect::<Vec<_>>().join(">"))
            .collect();
        written.sort();
        written.dedup();
        feature["properties"][RESTRICTION_KEY] = json!(written.join(" "));
    }
}

/// A piece of a way and the two OSM nodes it runs between.
type Span = (String, u64, u64);

/// The way across a `via`-way restriction: the junction `from` meets it at, the pieces crossed,
/// and the junction `to` leaves from.
type Crossing = (u64, Vec<Span>, u64);

/// The way across a `via`-way restriction: where `from` meets it, the pieces crossed, where `to`
/// leaves it.
///
/// The pieces are walked as an undirected graph rather than trusted in the order OSM listed them:
/// a relation's members carry no guaranteed order, and a way split at a junction contributes
/// several pieces whose order is this importer's own. The shortest crossing is taken, since a
/// restriction describes one manoeuvre and a longer walk through the same pieces is a different
/// one.
fn cross(pieces: &[Span], touching: &BTreeMap<u64, Vec<(u64, String)>>, from: u64, to: u64) -> Option<Crossing> {
    let meets = |node: u64, way: u64| {
        touching.get(&node).is_some_and(|pieces| pieces.iter().any(|(candidate, _)| *candidate == way))
    };
    let ends: Vec<u64> = pieces.iter().flat_map(|(_, first, last)| [*first, *last]).collect();
    // Breadth-first from every junction where `from` reaches the via, so the first arrival at a
    // junction `to` leaves from is the shortest crossing.
    let mut queue: std::collections::VecDeque<u64> = ends.iter().copied().filter(|node| meets(*node, from)).collect();
    let mut came_from: BTreeMap<u64, (String, u64, u64, u64)> = BTreeMap::new();
    let mut seen: Vec<u64> = queue.iter().copied().collect();
    while let Some(node) = queue.pop_front() {
        if meets(node, to) && came_from.contains_key(&node) {
            // Walk back to the junction the crossing started at, collecting the pieces in order.
            let mut chain = Vec::new();
            let mut at = node;
            while let Some((name, first, last, previous)) = came_from.get(&at).cloned() {
                chain.push((name, first, last));
                at = previous;
            }
            chain.reverse();
            return Some((at, chain, node));
        }
        for (name, first, last) in pieces {
            let next = if *first == node {
                *last
            } else if *last == node {
                *first
            } else {
                continue;
            };
            if seen.contains(&next) {
                continue;
            }
            seen.push(next);
            came_from.insert(next, (name.clone(), *first, *last, node));
            queue.push_back(next);
        }
    }
    None
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
