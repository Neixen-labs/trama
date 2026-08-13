// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Mapbox Vector Tiles, per SPEC 9: export-only, one file per `GEOM` record.
//!
//! This is the exit that needs no TRAMA at the other end. A `.mvt` pyramid is what a plain
//! MapLibre or Mapbox client already knows how to draw, so a network can be published without
//! anyone installing this project — which is the whole of the anti-lock-in claim, stated in the
//! format someone else's stack speaks.
//!
//! MVT is protobuf, but the slice of protobuf it uses is small enough to write by hand: varints,
//! length-delimited fields, and packed uint32 arrays. A code generator would be a build-time
//! dependency and a schema file to emit four message types.
//!
//! What is lost is in SPEC 9 and is not small: CSR topology, traversal order across tiles,
//! nullable typing, channel declarations. A tile is a picture of a network, not a network.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use trama_format::{Graph, Rows, Section, parse_tile};

/// MVT works in a 4096-unit tile space; SPEC 3.1 quantizes to 65535.
const EXTENT: u32 = 4096;
const QUANTIZED: u32 = 65535;

/// Where a node sits: the `z/x/y` of the tile holding it, and its quantized position in it.
type Placement = ((u32, u32, u32), (u16, u16));

/// Writes one `{z}/{x}/{y}.mvt` per `GEOM` record under `destination`.
pub fn write(
    sections: &[Section],
    graph: &Graph,
    node_rows: &Rows,
    edge_rows: &Rows,
    destination: &Path,
) -> Result<usize, String> {
    // Which tile holds each node, and where in it: the first vertex of an edge's first piece is
    // its source, the last vertex of its last piece its target. Read from the references rather
    // than from the tile paths, because a path's own ends may be tile-boundary crossings.
    let mut nodes: BTreeMap<u32, Placement> = BTreeMap::new();
    for edge in &graph.edges {
        let start = edge.reference_start as usize;
        let references = &graph.references[start..start + edge.reference_count as usize];
        let (Some(first), Some(last)) = (references.first(), references.last()) else { continue };
        for (reference, node, take_last) in [(first, edge.source, false), (last, edge.target, true)] {
            let section = sections.get(reference.directory_index as usize).ok_or("a reference names no section")?;
            let paths = parse_tile(&section.payload)?;
            let path = paths.get(reference.path_index as usize).ok_or("a reference names no path")?;
            // `direction` is the traversal sense: reversed, an edge starts at its path's end.
            let reversed = reference.direction < 0;
            let at_end = take_last != reversed;
            let vertex = if at_end { path.vertices.last() } else { path.vertices.first() };
            if let Some(vertex) = vertex {
                nodes.insert(node, (section.key, *vertex));
            }
        }
    }

    let mut written = 0;
    for section in sections {
        if &section.kind != b"GEOM" {
            continue;
        }
        let (z, x, y) = section.key;
        let paths = parse_tile(&section.payload)?;

        let mut edges_layer = Layer::new("edges");
        for path in &paths {
            let edge = graph.edges.get(path.edge_index as usize).ok_or("a path names no edge")?;
            edges_layer.feature(edge.id, 2, &line_geometry(&path.vertices), row(edge_rows, edge.property_row));
        }

        let mut nodes_layer = Layer::new("nodes");
        for (node_index, (key, vertex)) in &nodes {
            if *key != section.key {
                continue;
            }
            let node = graph.nodes.get(*node_index as usize).ok_or("a node index is out of range")?;
            nodes_layer.feature(node.id, 1, &point_geometry(*vertex), row(node_rows, node.property_row));
        }

        let mut tile = Vec::new();
        for layer in [edges_layer, nodes_layer] {
            if layer.features > 0 {
                field(&mut tile, 3, &layer.finish());
            }
        }
        if tile.is_empty() {
            continue;
        }
        let directory = destination.join(z.to_string()).join(x.to_string());
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        std::fs::write(directory.join(format!("{y}.mvt")), &tile).map_err(|error| error.to_string())?;
        written += 1;
    }
    Ok(written)
}

fn row(rows: &Rows, at: u32) -> &BTreeMap<String, Value> {
    static EMPTY: std::sync::OnceLock<BTreeMap<String, Value>> = std::sync::OnceLock::new();
    rows.get(at as usize).unwrap_or_else(|| EMPTY.get_or_init(BTreeMap::new))
}

/// A layer under construction, with the key and value dictionaries MVT features index into.
struct Layer {
    name: String,
    features: usize,
    body: Vec<u8>,
    keys: Vec<String>,
    values: Vec<Value>,
}

impl Layer {
    fn new(name: &str) -> Self {
        Self { name: name.to_string(), features: 0, body: Vec::new(), keys: Vec::new(), values: Vec::new() }
    }

    fn feature(&mut self, id: u64, kind: u64, geometry: &[u32], properties: &BTreeMap<String, Value>) {
        let mut tags: Vec<u32> = Vec::new();
        for (key, value) in properties {
            // A property whose type MVT cannot carry is left out rather than stringified: a tile
            // is allowed to hold less than the container, not something the container never said.
            if !matches!(value, Value::String(_) | Value::Number(_) | Value::Bool(_)) {
                continue;
            }
            tags.push(index_of(&mut self.keys, key.clone()) as u32);
            tags.push(index_of(&mut self.values, value.clone()) as u32);
        }

        let mut encoded = Vec::new();
        varint_field(&mut encoded, 1, id);
        field(&mut encoded, 2, &packed(&tags));
        varint_field(&mut encoded, 3, kind);
        field(&mut encoded, 4, &packed(geometry));
        field(&mut self.body, 2, &encoded);
        self.features += 1;
    }

    fn finish(self) -> Vec<u8> {
        let mut layer = Vec::new();
        field(&mut layer, 1, self.name.as_bytes());
        layer.extend_from_slice(&self.body);
        for key in &self.keys {
            field(&mut layer, 3, key.as_bytes());
        }
        for value in &self.values {
            field(&mut layer, 4, &encode_value(value));
        }
        varint_field(&mut layer, 5, u64::from(EXTENT));
        // Version is field 15 and MUST be 2, which is the one field a reader checks first.
        varint_field(&mut layer, 15, 2);
        layer
    }
}

fn index_of<T: PartialEq>(table: &mut Vec<T>, item: T) -> usize {
    match table.iter().position(|existing| *existing == item) {
        Some(at) => at,
        None => {
            table.push(item);
            table.len() - 1
        }
    }
}

/// A `Tile.Value`: one field set, chosen by what the property actually is.
fn encode_value(value: &Value) -> Vec<u8> {
    let mut encoded = Vec::new();
    match value {
        Value::String(text) => field(&mut encoded, 1, text.as_bytes()),
        Value::Bool(flag) => varint_field(&mut encoded, 7, u64::from(*flag)),
        Value::Number(number) if number.is_i64() => {
            // Field 6 is sint64: zigzag, so a negative integer costs one byte rather than ten.
            varint_field(&mut encoded, 6, zigzag64(number.as_i64().unwrap_or(0)));
        }
        Value::Number(number) => {
            encoded.push(3 << 3 | 1); // field 3, 64-bit: double
            encoded.extend_from_slice(&number.as_f64().unwrap_or(0.0).to_le_bytes());
        }
        _ => {}
    }
    encoded
}

/// MoveTo the first vertex, then LineTo the rest, in tile-relative deltas.
fn line_geometry(vertices: &[(u16, u16)]) -> Vec<u32> {
    let mut commands = Vec::new();
    if vertices.len() < 2 {
        return commands;
    }
    let scaled: Vec<(i32, i32)> = vertices.iter().map(|vertex| scale(*vertex)).collect();
    commands.push(command(1, 1));
    commands.push(zigzag(scaled[0].0));
    commands.push(zigzag(scaled[0].1));
    commands.push(command(2, scaled.len() as u32 - 1));
    for pair in scaled.windows(2) {
        commands.push(zigzag(pair[1].0 - pair[0].0));
        commands.push(zigzag(pair[1].1 - pair[0].1));
    }
    commands
}

fn point_geometry(vertex: (u16, u16)) -> Vec<u32> {
    let (x, y) = scale(vertex);
    vec![command(1, 1), zigzag(x), zigzag(y)]
}

/// SPEC 9: `round(q * 4096 / 65535)`.
fn scale(vertex: (u16, u16)) -> (i32, i32) {
    let convert = |value: u16| ((u32::from(value) * EXTENT + QUANTIZED / 2) / QUANTIZED) as i32;
    (convert(vertex.0), convert(vertex.1))
}

fn command(id: u32, count: u32) -> u32 {
    (id & 0x7) | (count << 3)
}

fn zigzag(value: i32) -> u32 {
    ((value << 1) ^ (value >> 31)) as u32
}

fn zigzag64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

// --- the protobuf slice this needs ---

fn varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A length-delimited field: wire type 2.
fn field(out: &mut Vec<u8>, number: u64, payload: &[u8]) {
    varint(out, number << 3 | 2);
    varint(out, payload.len() as u64);
    out.extend_from_slice(payload);
}

/// A varint field: wire type 0.
fn varint_field(out: &mut Vec<u8>, number: u64, value: u64) {
    varint(out, number << 3);
    varint(out, value);
}

fn packed(values: &[u32]) -> Vec<u8> {
    let mut out = Vec::new();
    for value in values {
        varint(&mut out, u64::from(*value));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_grow_a_byte_every_seven_bits() {
        let mut out = Vec::new();
        varint(&mut out, 0);
        varint(&mut out, 127);
        varint(&mut out, 128);
        varint(&mut out, 300);
        assert_eq!(out, vec![0x00, 0x7f, 0x80, 0x01, 0xac, 0x02]);
    }

    #[test]
    fn a_line_becomes_a_moveto_and_one_lineto_run() {
        // Two vertices at the tile's corners: 0 and 65535 scale to 0 and 4096.
        let commands = line_geometry(&[(0, 0), (65535, 65535)]);
        assert_eq!(commands[0], command(1, 1), "MoveTo, one pair");
        assert_eq!((commands[1], commands[2]), (0, 0));
        assert_eq!(commands[3], command(2, 1), "LineTo, one pair");
        assert_eq!((commands[4], commands[5]), (zigzag(4096), zigzag(4096)));
    }

    #[test]
    fn the_quantization_maps_onto_the_extent_at_both_ends() {
        assert_eq!(scale((0, 0)), (0, 0));
        assert_eq!(scale((65535, 65535)), (4096, 4096));
        assert_eq!(scale((32768, 32768)), (2048, 2048));
    }
}
