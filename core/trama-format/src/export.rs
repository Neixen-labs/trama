// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! GeoJSON export, per SPEC 9.
//!
//! Coordinates come back through the inverse of the section 3.1 quantization, so they carry
//! the precision the file stores and not the precision the source had.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::read::{GeometryReference, parse_graph, read_sections};

const WORLD: f64 = 40075016.68557849;
const EXTENT: f64 = 65535.0;

/// Decoded geometry by directory index, with the tile key each payload belongs to.
type Tiles<'a> = BTreeMap<usize, (&'a (u32, u32, u32), Vec<Vec<(u16, u16)>>)>;

pub struct Export {
    pub nodes: Value,
    pub edges: Value,
}

/// Node and edge FeatureCollections, ready to write beside each other.
pub fn export(data: &[u8]) -> Result<Export, String> {
    let sections = read_sections(data)?;
    let graph = sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?;
    let properties = sections.iter().find(|s| &s.kind == b"PROP").ok_or("container is missing a PROP section")?;
    let geometry: Tiles = sections
        .iter()
        .enumerate()
        .filter(|(_index, section)| &section.kind == b"GEOM")
        .map(|(index, section)| Ok((index, (&section.key, parse_geometry(&section.payload)?))))
        .collect::<Result<_, String>>()?;

    let parsed = parse_graph(&graph.payload)?;
    let (node_rows, edge_rows) = parse_properties(&properties.payload)?;

    let mut positions: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    let mut edge_features = Vec::with_capacity(parsed.edges.len());
    for edge in &parsed.edges {
        let start = edge.reference_start as usize;
        let references = &parsed.references[start..start + edge.reference_count as usize];
        let metres = edge_coordinates(references, &geometry)?;
        let coordinates: Vec<(f64, f64)> = metres.iter().map(|point| wgs84(*point)).collect();
        positions.insert(edge.source, coordinates[0]);
        positions.insert(edge.target, coordinates[coordinates.len() - 1]);
        let mut exported = feature(
            json!({"type": "LineString", "coordinates": coordinates.iter().map(|(x, y)| json!([x, y])).collect::<Vec<Value>>()}),
            edge.id,
            edge_rows.get(edge.property_row as usize),
        );
        // SPEC 9: written only when set, so a file with no directed edge round-trips unchanged.
        if edge.directed {
            exported["properties"]["_trama_directed"] = Value::Bool(true);
        }
        edge_features.push(exported);
    }

    let node_features: Vec<Value> = parsed
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            positions.get(&(index as u32)).map(|(x, y)| {
                feature(
                    json!({"type": "Point", "coordinates": [x, y]}),
                    node.id,
                    node_rows.get(node.property_row as usize),
                )
            })
        })
        .collect();

    Ok(Export {
        nodes: json!({"type": "FeatureCollection", "features": node_features}),
        edges: json!({"type": "FeatureCollection", "features": edge_features}),
    })
}

fn feature(geometry: Value, id: u64, row: Option<&BTreeMap<String, Value>>) -> Value {
    let mut properties = Map::new();
    properties.insert("_trama_id".into(), Value::String(id.to_string()));
    for (key, value) in row.into_iter().flatten() {
        properties.insert(key.clone(), value.clone());
    }
    json!({"type": "Feature", "geometry": geometry, "properties": Value::Object(properties)})
}

/// Every edge's centerline in `EPSG:3857` metres, indexed by edge index.
///
/// This is what a solver costing a traversal needs, and it is the same reconstruction the export
/// does before projecting to WGS 84 — geometry rather than domain, so it belongs to the format.
/// Lengths carry the precision the file stores: section 3.1 quantizes to about 4 cm at `z14`.
/// The length of every edge, in the metres of the projection the container stores.
///
/// ponytail: Web Mercator metres, not ground metres — they run long by `1/cos(latitude)`, about
/// 30% at Madrid's. Every consumer so far compares lengths within one network, where the factor
/// cancels, and correcting it is #132 rather than silent.
pub fn edge_lengths(data: &[u8]) -> Result<Vec<f64>, String> {
    Ok(edge_paths(data)?
        .iter()
        .map(|path| path.windows(2).map(|pair| (pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1)).sum())
        .collect())
}

pub fn edge_paths(data: &[u8]) -> Result<Vec<Vec<(f64, f64)>>, String> {
    let sections = read_sections(data)?;
    let graph = sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?;
    let geometry: Tiles = sections
        .iter()
        .enumerate()
        .filter(|(_index, section)| &section.kind == b"GEOM")
        .map(|(index, section)| Ok((index, (&section.key, parse_geometry(&section.payload)?))))
        .collect::<Result<_, String>>()?;
    let parsed = parse_graph(&graph.payload)?;
    parsed
        .edges
        .iter()
        .map(|edge| {
            let start = edge.reference_start as usize;
            edge_coordinates(&parsed.references[start..start + edge.reference_count as usize], &geometry)
        })
        .collect()
}

/// One edge's centerline in `EPSG:3857` metres, in traversal order across the tiles it spans.
fn edge_coordinates(references: &[GeometryReference], geometry: &Tiles) -> Result<Vec<(f64, f64)>, String> {
    let mut coordinates: Vec<(f64, f64)> = Vec::new();
    for reference in references {
        let (key, paths) =
            geometry.get(&(reference.directory_index as usize)).ok_or("edge references a section that is not GEOM")?;
        let path = paths.get(reference.path_index as usize).ok_or("edge references a missing path")?;
        let mut piece: Vec<(f64, f64)> = path.iter().map(|point| dequantize(*point, **key)).collect();
        if reference.direction < 0 {
            piece.reverse();
        }
        // Consecutive pieces meet at a tile boundary, so the shared vertex is already there.
        if coordinates.is_empty() {
            coordinates.extend(piece);
        } else {
            coordinates.extend(piece.drain(1..));
        }
    }
    if coordinates.is_empty() {
        return Err("edge has no geometry".into());
    }
    Ok(coordinates)
}

fn parse_geometry(payload: &[u8]) -> Result<Vec<Vec<(u16, u16)>>, String> {
    let u32_at = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().unwrap());
    if payload.len() < 32 {
        return Err("geometry section is shorter than its header".into());
    }
    let path_count = u32_at(0) as usize;
    let paths_offset = u32_at(16) as usize;
    let vertices_offset = u32_at(20) as usize;
    (0..path_count)
        .map(|index| {
            let at = paths_offset + index * 16;
            if at + 16 > payload.len() {
                return Err("geometry path runs past the section".into());
            }
            let first_vertex = u32_at(at + 4) as usize;
            let vertex_count = u32_at(at + 8) as usize;
            let start = vertices_offset + first_vertex * 4;
            if start + vertex_count * 4 > payload.len() {
                return Err("geometry vertices run past the section".into());
            }
            Ok((0..vertex_count)
                .map(|vertex| {
                    let at = start + vertex * 4;
                    (
                        u16::from_le_bytes(payload[at..at + 2].try_into().unwrap()),
                        u16::from_le_bytes(payload[at + 2..at + 4].try_into().unwrap()),
                    )
                })
                .collect())
        })
        .collect()
}

pub type Rows = Vec<BTreeMap<String, Value>>;

/// Every edge property row, addressed by an edge's `property_row`.
///
/// A solver costing a traversal by something the source measured — a speed limit, a diameter —
/// needs the columns, and only the exporter could read them. Typed values, no domain meaning:
/// what a key means belongs to whoever wrote it.
pub fn edge_properties(data: &[u8]) -> Result<Rows, String> {
    let sections = read_sections(data)?;
    let properties = sections.iter().find(|s| &s.kind == b"PROP").ok_or("container is missing a PROP section")?;
    Ok(parse_properties(&properties.payload)?.1)
}

fn parse_properties(payload: &[u8]) -> Result<(Rows, Rows), String> {
    let u32_at = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().unwrap());
    if payload.len() < 40 {
        return Err("property section is shorter than its header".into());
    }
    let node_columns = u32_at(12) as usize;
    let edge_columns = u32_at(16) as usize;
    let keys = read_strings(payload, u32_at(20) as usize)?;
    let strings = read_strings(payload, u32_at(24) as usize)?;
    Ok((
        read_columns(payload, u32_at(32) as usize, node_columns, &keys, &strings)?,
        read_columns(payload, u32_at(36) as usize, edge_columns, &keys, &strings)?,
    ))
}

fn read_columns(
    payload: &[u8],
    columns_offset: usize,
    count: usize,
    keys: &[String],
    strings: &[String],
) -> Result<Rows, String> {
    let u32_at = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().unwrap());
    let mut rows: Rows = Vec::new();
    for column in 0..count {
        let at = columns_offset + column * 20;
        if at + 20 > payload.len() {
            return Err("property column runs past the section".into());
        }
        let key_id = u32_at(at) as usize;
        let value_type = payload[at + 5];
        let entity_count = u32_at(at + 8) as usize;
        let presence_offset = u32_at(at + 12) as usize;
        let values_offset = u32_at(at + 16) as usize;
        while rows.len() < entity_count {
            rows.push(BTreeMap::new());
        }
        let key = keys.get(key_id).ok_or("property column names a missing key")?;
        let mut dense = 0usize;
        for entity in 0..entity_count {
            if payload[presence_offset + entity / 8] >> (entity % 8) & 1 == 0 {
                continue;
            }
            rows[entity].insert(key.clone(), read_value(payload, values_offset, dense, value_type, strings)?);
            dense += 1;
        }
    }
    Ok(rows)
}

fn read_value(
    payload: &[u8],
    values_offset: usize,
    dense: usize,
    value_type: u8,
    strings: &[String],
) -> Result<Value, String> {
    match value_type {
        1 => {
            let at = values_offset + dense * 8;
            Ok(json!(f64::from_le_bytes(payload[at..at + 8].try_into().unwrap())))
        }
        2 => {
            let at = values_offset + dense * 8;
            Ok(json!(i64::from_le_bytes(payload[at..at + 8].try_into().unwrap())))
        }
        3 => {
            let at = values_offset + dense * 4;
            let index = u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()) as usize;
            Ok(Value::String(strings.get(index).ok_or("string value names a missing entry")?.clone()))
        }
        4 => Ok(Value::Bool(payload[values_offset + dense / 8] >> (dense % 8) & 1 != 0)),
        other => Err(format!("unsupported v0 property type {other}")),
    }
}

fn read_strings(payload: &[u8], offset: usize) -> Result<Vec<String>, String> {
    if offset + 4 > payload.len() {
        return Err("string table runs past the section".into());
    }
    let count = u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
    let mut values = Vec::with_capacity(count);
    let mut at = offset + 4;
    for _ in 0..count {
        if at + 4 > payload.len() {
            return Err("string table runs past the section".into());
        }
        let length = u32::from_le_bytes(payload[at..at + 4].try_into().unwrap()) as usize;
        if at + 4 + length > payload.len() {
            return Err("string table runs past the section".into());
        }
        values.push(String::from_utf8_lossy(&payload[at + 4..at + 4 + length]).into_owned());
        at += 4 + length;
    }
    Ok(values)
}

fn dequantize(point: (u16, u16), tile: (u32, u32, u32)) -> (f64, f64) {
    let width = WORLD / f64::from(1u32 << tile.0);
    (
        -WORLD / 2.0 + f64::from(tile.1) * width + f64::from(point.0) / EXTENT * width,
        WORLD / 2.0 - f64::from(tile.2) * width - f64::from(point.1) / EXTENT * width,
    )
}

fn wgs84(point: (f64, f64)) -> (f64, f64) {
    let longitude = point.0 * 180.0 / (WORLD / 2.0);
    let latitude =
        (2.0 * (point.1 * std::f64::consts::PI / (WORLD / 2.0)).exp().atan() - std::f64::consts::PI / 2.0).to_degrees();
    (longitude, latitude)
}
