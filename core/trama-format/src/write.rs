// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A port of the TRAMA v0 compiler, kept byte-identical to the Python one.
//!
//! Every ordering, rounding and string form here exists to match `compiler/`. Where the two
//! could disagree — sort order, float formatting, hash input — the Python spelling is the
//! authority and this file follows it.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"TRAMA\0\0\0";
const EXTENT: f64 = 65535.0;
const WORLD: f64 = 40075016.68557849;
const ID_KEY: &str = "_trama_id";
const COMPRESSION_LEVEL: i32 = 19;

pub struct Extra {
    pub owner: String,
    pub media_type: String,
    pub payload: Vec<u8>,
}

type Point = (f64, f64);
type TileKey = (u32, u32, u32);
/// An edge on its way into the file: identity, endpoints, geometry per tile, and its row.
type EdgeRecord = (u64, u64, u64, Vec<(TileKey, Vec<Point>)>, BTreeMap<String, Value>);
type TilePaths = BTreeMap<TileKey, Vec<(u32, Vec<(u16, u16)>)>>;
type ColumnGroup<'a> = (u8, &'a [&'a BTreeMap<String, Value>], Vec<String>);

pub fn compile(features: &[Value], channels: &[Value], extras: &[Extra]) -> Result<Vec<u8>, String> {
    let lines: Vec<&Value> = features.iter().filter(|f| geometry_type(f) == "LineString").collect();
    let points: Vec<&Value> = features.iter().filter(|f| geometry_type(f) == "Point").collect();
    if lines.is_empty() || lines.len() + points.len() != features.len() {
        return Err("v0 compiler slice requires one LineString feature".into());
    }

    let mut projected_lines: Vec<(String, Vec<Point>)> = Vec::with_capacity(lines.len());
    for (index, feature) in lines.iter().enumerate() {
        let coordinates = feature["geometry"]["coordinates"].as_array().ok_or("LineString requires coordinates")?;
        if coordinates.len() < 2 {
            return Err("LineString requires at least two coordinates".into());
        }
        let path = coordinates.iter().map(|pair| web_mercator(number(&pair[0]), number(&pair[1]))).collect();
        projected_lines.push((feature_id(feature, index), path));
    }

    let mut node_ids: BTreeMap<(u64, u64), u64> = BTreeMap::new();
    for (_id, path) in &projected_lines {
        for point in [path[0], path[path.len() - 1]] {
            let cell = node_cell(point);
            node_ids.insert(cell, stable_id(&format!("node:{},{}", cell.0, cell.1)));
        }
    }
    let mut node_properties: BTreeMap<(u64, u64), BTreeMap<String, Value>> = BTreeMap::new();
    for feature in &points {
        let coordinates = feature["geometry"]["coordinates"].as_array().ok_or("Point requires coordinates")?;
        let cell = node_cell(web_mercator(number(&coordinates[0]), number(&coordinates[1])));
        if let Some(declared) = declared_id(feature)? {
            node_ids.insert(cell, declared);
        }
        node_properties.insert(cell, row_of(feature));
    }

    // Edges are sorted by stable id, as SPEC 4 requires of the array they become.
    let mut ordered: Vec<EdgeRecord> = Vec::new();
    for (feature, (id, path)) in lines.iter().zip(&projected_lines) {
        let edge_id = match declared_id(feature)? {
            Some(declared) => declared,
            None => stable_id(&format!("edge:{}", id)),
        };
        let source = *node_ids.get(&node_cell(path[0])).ok_or("missing source node")?;
        let target = *node_ids.get(&node_cell(path[path.len() - 1])).ok_or("missing target node")?;
        ordered.push((edge_id, source, target, split_by_tile(path), row_of(feature)));
    }
    ordered.sort_by_key(|record| record.0);
    if ordered.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err("GeoJSON feature IDs must be unique".into());
    }

    let node_order: Vec<u64> = {
        let mut identities: BTreeSet<u64> = BTreeSet::new();
        for (_id, source, target, _pieces, _row) in &ordered {
            identities.insert(*source);
            identities.insert(*target);
        }
        identities.into_iter().collect()
    };
    let node_index: BTreeMap<u64, u32> = node_order.iter().enumerate().map(|(index, id)| (*id, index as u32)).collect();
    let rows_by_id: BTreeMap<u64, &BTreeMap<String, Value>> =
        node_properties.iter().filter_map(|(cell, row)| node_ids.get(cell).map(|id| (*id, row))).collect();
    let empty = BTreeMap::new();
    let node_rows: Vec<&BTreeMap<String, Value>> =
        node_order.iter().map(|id| *rows_by_id.get(id).unwrap_or(&&empty)).collect();
    let edge_rows: Vec<&BTreeMap<String, Value>> = ordered.iter().map(|record| &record.4).collect();

    let tiles: Vec<TileKey> = {
        let mut keys: BTreeSet<TileKey> = BTreeSet::new();
        for record in &ordered {
            for (tile, _piece) in &record.3 {
                keys.insert(*tile);
            }
        }
        keys.into_iter().collect()
    };
    let tile_index: BTreeMap<TileKey, u32> =
        tiles.iter().enumerate().map(|(index, key)| (*key, index as u32)).collect();
    let mut tile_paths: TilePaths = tiles.iter().map(|key| (*key, Vec::new())).collect();
    let mut geometry_refs: Vec<Vec<(u32, u32)>> = Vec::with_capacity(ordered.len());
    for (edge_index, record) in ordered.iter().enumerate() {
        let mut refs = Vec::new();
        for (tile, piece) in &record.3 {
            let paths = tile_paths.get_mut(tile).ok_or("unknown tile")?;
            refs.push((tile_index[tile], paths.len() as u32));
            paths.push((edge_index as u32, piece.iter().map(|point| quantize(*point, *tile)).collect()));
        }
        geometry_refs.push(refs);
    }

    let edges: Vec<(u64, u32, u32)> =
        ordered.iter().map(|record| (record.0, node_index[&record.1], node_index[&record.2])).collect();

    let mut decoded: Vec<(&[u8; 4], u32, TileKey, Vec<u8>)> = Vec::new();
    for tile in &tiles {
        decoded.push((b"GEOM", 1, *tile, geometry_section(&tile_paths[tile])));
    }
    decoded.push((b"GRPH", 1, (0, 0, 0), graph_section(&edges, &node_order, &geometry_refs)));
    decoded.push((b"PROP", 1, (0, 0, 0), property_section(&node_rows, &edge_rows)?));
    decoded.push((b"STCH", 1, (0, 0, 0), state_channel_section(channels)?));
    for payload in extra_sections(extras)? {
        decoded.push((b"XTRA", 0, (0, 0, 0), payload));
    }

    let mut hasher = Sha256::new();
    for (_kind, _flags, _key, payload) in &decoded {
        hasher.update(payload);
    }
    let file_uuid = &hasher.finalize()[..16].to_vec();

    let stored: Vec<Vec<u8>> = decoded
        .iter()
        .map(|(_kind, _flags, _key, payload)| zstd::bulk::compress(payload, COMPRESSION_LEVEL).unwrap())
        .collect();
    let header_size = 64u64;
    let directory_bytes = decoded.len() as u64 * 64;
    let mut offset = header_size + directory_bytes;
    let mut offsets = Vec::with_capacity(stored.len());
    for compressed in &stored {
        offsets.push(offset);
        offset += compressed.len() as u64;
    }

    let mut file = Vec::with_capacity(offset as usize);
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&[0u16, 1, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
    file.extend_from_slice(&[0u16, 1, 0].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>());
    file.extend_from_slice(&64u32.to_le_bytes());
    file.extend_from_slice(&64u64.to_le_bytes());
    file.extend_from_slice(&(decoded.len() as u32).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&offset.to_le_bytes());
    file.extend_from_slice(file_uuid);
    for (index, (kind, flags, key, payload)) in decoded.iter().enumerate() {
        file.extend_from_slice(*kind);
        file.extend_from_slice(&flags.to_le_bytes());
        file.extend_from_slice(&key.0.to_le_bytes());
        file.extend_from_slice(&key.1.to_le_bytes());
        file.extend_from_slice(&key.2.to_le_bytes());
        file.extend_from_slice(&offsets[index].to_le_bytes());
        file.extend_from_slice(&(stored[index].len() as u64).to_le_bytes());
        file.extend_from_slice(&(payload.len() as u64).to_le_bytes());
        file.extend_from_slice(&crate::crc32c(payload).to_le_bytes());
        file.extend_from_slice(&1u16.to_le_bytes());
        file.extend_from_slice(&[0u8, 0]);
        file.extend_from_slice(&[0u8; 12]);
    }
    for compressed in &stored {
        file.extend_from_slice(compressed);
    }
    Ok(file)
}

fn geometry_type(feature: &Value) -> &str {
    feature.get("geometry").and_then(|g| g.get("type")).and_then(Value::as_str).unwrap_or("")
}

fn feature_id(feature: &Value, index: usize) -> String {
    match feature.get("id") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => {
            if let Some(integer) = number.as_i64() {
                integer.to_string()
            } else {
                python_float(number.as_f64().unwrap_or_default())
            }
        }
        _ => format!("edge-{index}"),
    }
}

/// Python's `str(float)`: a trailing `.0` where Rust would print an integer.
fn python_float(value: f64) -> String {
    let rendered = format!("{value}");
    if rendered.contains(['.', 'e', 'n', 'i']) { rendered } else { format!("{rendered}.0") }
}

fn row_of(feature: &Value) -> BTreeMap<String, Value> {
    feature
        .get("properties")
        .and_then(Value::as_object)
        .map(|row| {
            row.iter()
                .filter(|(key, _value)| key.as_str() != ID_KEY)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
        .unwrap_or_default()
}

fn declared_id(feature: &Value) -> Result<Option<u64>, String> {
    let declared = match feature.get("properties").and_then(|row| row.get(ID_KEY)) {
        None | Some(Value::Null) => return Ok(None),
        Some(value) => value,
    };
    let text = match declared {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        other => return Err(format!("{ID_KEY} must be a decimal integer, got {other}")),
    };
    text.parse::<u64>().map(Some).map_err(|_| format!("{ID_KEY} must fit in u64, got '{text}'"))
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap_or_default()
}

fn stable_id(value: &str) -> u64 {
    let digest = Sha256::digest(value.as_bytes());
    u64::from_le_bytes(digest[..8].try_into().unwrap())
}

fn web_mercator(longitude: f64, latitude: f64) -> Point {
    let latitude = latitude.clamp(-85.05112878, 85.05112878);
    let x = longitude * 20037508.342789244 / 180.0;
    let y = ((90.0 + latitude) * std::f64::consts::PI / 360.0).tan().ln() / (std::f64::consts::PI / 180.0);
    (x, y * 20037508.342789244 / 180.0)
}

fn tile_key(point: Point) -> TileKey {
    let tiles = 1u32 << 14;
    let x = (((point.0 + WORLD / 2.0) / WORLD * tiles as f64) as i64).clamp(0, tiles as i64 - 1);
    let y = (((WORLD / 2.0 - point.1) / WORLD * tiles as f64) as i64).clamp(0, tiles as i64 - 1);
    (14, x as u32, y as u32)
}

fn quantize(point: Point, tile: TileKey) -> (u16, u16) {
    let width = WORLD / (1u32 << tile.0) as f64;
    let min_x = -WORLD / 2.0 + tile.1 as f64 * width;
    let max_y = WORLD / 2.0 - tile.2 as f64 * width;
    (
        round_half_even((point.0 - min_x) / width * EXTENT).clamp(0.0, EXTENT) as u16,
        round_half_even((max_y - point.1) / width * EXTENT).clamp(0.0, EXTENT) as u16,
    )
}

/// Python's `round`: ties go to the even neighbour, unlike Rust's `f64::round`.
fn round_half_even(value: f64) -> f64 {
    let floor = value.floor();
    let fraction = value - floor;
    let upward = fraction > 0.5 || (fraction == 0.5 && (floor as i64) % 2 != 0);
    if upward { floor + 1.0 } else { floor }
}

fn node_cell(point: Point) -> (u64, u64) {
    let tile = tile_key(point);
    let (x, y) = quantize(point, tile);
    (tile.1 as u64 * EXTENT as u64 + x as u64, tile.2 as u64 * EXTENT as u64 + y as u64)
}

fn interpolate(start: Point, end: Point, fraction: f64) -> Point {
    (start.0 + (end.0 - start.0) * fraction, start.1 + (end.1 - start.1) * fraction)
}

fn boundary_crossings(start: Point, end: Point) -> Vec<f64> {
    let width = WORLD / (1u32 << 14) as f64;
    let mut crossings = Vec::new();
    for axis in 0..2 {
        let (from, to) = if axis == 0 { (start.0, end.0) } else { (start.1, end.1) };
        let span = to - from;
        if span == 0.0 {
            continue;
        }
        let (low, high) = if from < to { (from, to) } else { (to, from) };
        let first = (low / width).floor() as i64 + 1;
        let last = (high / width).ceil() as i64;
        for step in first..last {
            let fraction = (step as f64 * width - from) / span;
            if fraction > 0.0 && fraction < 1.0 {
                crossings.push(fraction);
            }
        }
    }
    crossings.sort_by(|a, b| a.partial_cmp(b).unwrap());
    crossings
}

fn split_by_tile(points: &[Point]) -> Vec<(TileKey, Vec<Point>)> {
    let mut pieces: Vec<(TileKey, Vec<Point>)> = Vec::new();
    for pair in points.windows(2) {
        let (start, end) = (pair[0], pair[1]);
        let mut cuts = vec![0.0];
        cuts.extend(boundary_crossings(start, end));
        cuts.push(1.0);
        for span in cuts.windows(2) {
            let (span_start, span_end) = (span[0], span[1]);
            let tile = tile_key(interpolate(start, end, (span_start + span_end) / 2.0));
            let piece_start = if span_start == 0.0 { start } else { interpolate(start, end, span_start) };
            let piece_end = if span_end == 1.0 { end } else { interpolate(start, end, span_end) };
            match pieces.last_mut() {
                Some(last) if last.0 == tile => last.1.push(piece_end),
                _ => pieces.push((tile, vec![piece_start, piece_end])),
            }
        }
    }
    pieces
}

fn geometry_section(paths: &[(u32, Vec<(u16, u16)>)]) -> Vec<u8> {
    let mut headers = Vec::new();
    let mut first_vertex = 0u32;
    for (edge_index, path) in paths {
        headers.extend_from_slice(&edge_index.to_le_bytes());
        headers.extend_from_slice(&first_vertex.to_le_bytes());
        headers.extend_from_slice(&(path.len() as u32).to_le_bytes());
        headers.extend_from_slice(&0u32.to_le_bytes());
        first_vertex += path.len() as u32;
    }
    let mut vertices = Vec::new();
    for (_edge_index, path) in paths {
        for (x, y) in path {
            vertices.extend_from_slice(&x.to_le_bytes());
            vertices.extend_from_slice(&y.to_le_bytes());
        }
    }
    let header_size = 32u32;
    let paths_offset = header_size;
    let vertices_offset = paths_offset + headers.len() as u32;
    let mesh_offset = vertices_offset + vertices.len() as u32;
    let mut section = Vec::new();
    for value in [paths.len() as u32, first_vertex, 0, 0, paths_offset, vertices_offset, mesh_offset, mesh_offset] {
        section.extend_from_slice(&value.to_le_bytes());
    }
    section.extend_from_slice(&headers);
    section.extend_from_slice(&vertices);
    section
}

fn identity_block(identities: &[u64]) -> Vec<u8> {
    let mut block = Vec::new();
    let mut previous = 0u64;
    for identity in identities {
        let mut gap = identity.wrapping_sub(previous);
        loop {
            let group = (gap & 0x7F) as u8;
            gap >>= 7;
            block.push(group | if gap != 0 { 0x80 } else { 0 });
            if gap == 0 {
                break;
            }
        }
        previous = *identity;
    }
    block
}

fn graph_section(edges: &[(u64, u32, u32)], node_order: &[u64], geometry_refs: &[Vec<(u32, u32)>]) -> Vec<u8> {
    let mut adjacency: Vec<Vec<(u32, i8)>> = vec![Vec::new(); node_order.len()];
    for (edge_index, (_id, source, target)) in edges.iter().enumerate() {
        adjacency[*source as usize].push((edge_index as u32, 1));
        adjacency[*target as usize].push((edge_index as u32, -1));
    }
    let adjacency_count: usize = adjacency.iter().map(Vec::len).sum();
    let ref_count: usize = geometry_refs.iter().map(Vec::len).sum();

    let header_size = 44u32;
    let nodes_offset = header_size;
    let edges_offset = nodes_offset + node_order.len() as u32 * 8;
    let csr_offset = edges_offset + edges.len() as u32 * 24;
    let adjacency_offset = csr_offset + (node_order.len() as u32 + 1) * 8;
    let refs_offset = adjacency_offset + adjacency_count as u32 * 8;
    let node_id_block = identity_block(node_order);
    let node_ids_offset = refs_offset + ref_count as u32 * 12;
    let edge_ids_offset = node_ids_offset + node_id_block.len() as u32;

    let mut section = Vec::new();
    for value in [
        node_order.len() as u32,
        edges.len() as u32,
        adjacency_count as u32,
        ref_count as u32,
        nodes_offset,
        edges_offset,
        csr_offset,
        adjacency_offset,
        refs_offset,
        node_ids_offset,
        edge_ids_offset,
    ] {
        section.extend_from_slice(&value.to_le_bytes());
    }
    for index in 0..node_order.len() as u32 {
        section.extend_from_slice(&index.to_le_bytes());
        section.extend_from_slice(&0u32.to_le_bytes());
    }
    let mut ref_start = 0u32;
    for (edge_index, ((_id, source, target), refs)) in edges.iter().zip(geometry_refs).enumerate() {
        for value in [*source, *target, edge_index as u32, ref_start, refs.len() as u32, 0] {
            section.extend_from_slice(&value.to_le_bytes());
        }
        ref_start += refs.len() as u32;
    }
    let mut running = 0u64;
    section.extend_from_slice(&running.to_le_bytes());
    for entries in &adjacency {
        running += entries.len() as u64;
        section.extend_from_slice(&running.to_le_bytes());
    }
    for entries in &adjacency {
        for (edge_index, direction) in entries {
            section.extend_from_slice(&edge_index.to_le_bytes());
            section.push(*direction as u8);
            section.extend_from_slice(&[0u8; 3]);
        }
    }
    for refs in geometry_refs {
        for (directory_index, path_index) in refs {
            section.extend_from_slice(&directory_index.to_le_bytes());
            section.extend_from_slice(&path_index.to_le_bytes());
            section.push(1);
            section.extend_from_slice(&[0u8; 3]);
        }
    }
    section.extend_from_slice(&node_id_block);
    section.extend_from_slice(&identity_block(&edges.iter().map(|edge| edge.0).collect::<Vec<u64>>()));
    section
}

fn value_type(value: &Value) -> Result<u8, String> {
    match value {
        Value::Bool(_) => Ok(4),
        Value::Number(number) if number.is_i64() || number.is_u64() => Ok(2),
        Value::Number(number) if number.as_f64().map(f64::is_finite).unwrap_or(false) => Ok(1),
        Value::String(_) => Ok(3),
        _ => Err("v0 properties support only finite numbers, strings, and booleans".into()),
    }
}

fn column_type(key: &str, values: &[&Value]) -> Result<u8, String> {
    let mut kinds: BTreeSet<u8> = BTreeSet::new();
    for value in values {
        kinds.insert(value_type(value)?);
    }
    if kinds.len() == 2 && kinds.contains(&1) && kinds.contains(&2) {
        return Ok(1);
    }
    if kinds.len() != 1 {
        return Err(format!("property '{key}' mixes conflicting types across features"));
    }
    Ok(*kinds.iter().next().unwrap())
}

fn packed_bits(set_indexes: &[usize], count: usize) -> Vec<u8> {
    let mut bits = vec![0u8; count.div_ceil(8)];
    for index in set_indexes {
        bits[index / 8] |= 1 << (index % 8);
    }
    bits
}

fn string_dictionary(values: &[String]) -> Vec<u8> {
    let mut block = (values.len() as u32).to_le_bytes().to_vec();
    for value in values {
        block.extend_from_slice(&(value.len() as u32).to_le_bytes());
        block.extend_from_slice(value.as_bytes());
    }
    block
}

fn property_section(
    node_rows: &[&BTreeMap<String, Value>],
    edge_rows: &[&BTreeMap<String, Value>],
) -> Result<Vec<u8>, String> {
    let groups: [(u8, &[&BTreeMap<String, Value>]); 2] = [(1, node_rows), (2, edge_rows)];
    let used: Vec<ColumnGroup> = groups
        .iter()
        .map(|(kind, rows)| {
            let mut keys: BTreeSet<String> = BTreeSet::new();
            for row in rows.iter() {
                for (key, value) in row.iter() {
                    if !value.is_null() {
                        keys.insert(key.clone());
                    }
                }
            }
            (*kind, *rows, keys.into_iter().collect())
        })
        .collect();
    let keys: Vec<String> = {
        let mut all: BTreeSet<String> = BTreeSet::new();
        for (_kind, _rows, group) in &used {
            all.extend(group.iter().cloned());
        }
        all.into_iter().collect()
    };
    let key_ids: BTreeMap<&String, u32> = keys.iter().enumerate().map(|(index, key)| (key, index as u32)).collect();
    let string_values: Vec<String> = {
        let mut all: BTreeSet<String> = BTreeSet::new();
        for (_kind, rows) in &groups {
            for row in rows.iter() {
                for value in row.values() {
                    if let Value::String(text) = value {
                        all.insert(text.clone());
                    }
                }
            }
        }
        all.into_iter().collect()
    };
    let string_ids: BTreeMap<&String, u32> =
        string_values.iter().enumerate().map(|(index, value)| (value, index as u32)).collect();

    let key_dictionary = string_dictionary(&keys);
    let value_dictionary = string_dictionary(&string_values);
    let enum_dictionary = 0u32.to_le_bytes().to_vec();
    let header_size = 40u32;
    let key_offset = header_size;
    let string_offset = key_offset + key_dictionary.len() as u32;
    let enum_offset = string_offset + value_dictionary.len() as u32;
    let columns_offset = enum_offset + enum_dictionary.len() as u32;
    let column_count: usize = used.iter().map(|(_kind, _rows, group)| group.len()).sum();
    let values_offset = columns_offset + column_count as u32 * 20;

    let mut columns = Vec::new();
    let mut bodies: Vec<u8> = Vec::new();
    for (kind, rows, group) in &used {
        let bitmap_bytes = rows.len().div_ceil(8);
        for key in group {
            let present: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_index, row)| row.get(key).map(|value| !value.is_null()).unwrap_or(false))
                .map(|(index, _row)| index)
                .collect();
            let values: Vec<&Value> = present.iter().map(|index| &rows[*index][key]).collect();
            let kind_of_column = column_type(key, &values)?;
            let presence_offset = values_offset + bodies.len() as u32;
            columns.extend_from_slice(&key_ids[key].to_le_bytes());
            columns.push(*kind);
            columns.push(kind_of_column);
            columns.extend_from_slice(&1u16.to_le_bytes());
            columns.extend_from_slice(&(rows.len() as u32).to_le_bytes());
            columns.extend_from_slice(&presence_offset.to_le_bytes());
            columns.extend_from_slice(&(presence_offset + bitmap_bytes as u32).to_le_bytes());
            bodies.extend_from_slice(&packed_bits(&present, rows.len()));
            match kind_of_column {
                4 => {
                    let set: Vec<usize> = values
                        .iter()
                        .enumerate()
                        .filter(|(_index, value)| value.as_bool().unwrap_or(false))
                        .map(|(index, _value)| index)
                        .collect();
                    bodies.extend_from_slice(&packed_bits(&set, values.len()));
                }
                3 => {
                    for value in &values {
                        let text = value.as_str().ok_or("property value is not a string")?.to_string();
                        bodies.extend_from_slice(&string_ids[&text].to_le_bytes());
                    }
                }
                2 => {
                    for value in &values {
                        bodies.extend_from_slice(
                            &value.as_i64().ok_or("property value is not an integer")?.to_le_bytes(),
                        );
                    }
                }
                _ => {
                    for value in &values {
                        bodies.extend_from_slice(
                            &value.as_f64().ok_or("property value is not a finite number")?.to_le_bytes(),
                        );
                    }
                }
            }
        }
    }

    let node_columns = used[0].2.len() as u32;
    let mut section = Vec::new();
    for value in [
        keys.len() as u32,
        string_values.len() as u32,
        0,
        node_columns,
        used[1].2.len() as u32,
        key_offset,
        string_offset,
        enum_offset,
        columns_offset,
        columns_offset + node_columns * 20,
    ] {
        section.extend_from_slice(&value.to_le_bytes());
    }
    section.extend_from_slice(&key_dictionary);
    section.extend_from_slice(&value_dictionary);
    section.extend_from_slice(&enum_dictionary);
    section.extend_from_slice(&columns);
    section.extend_from_slice(&bodies);
    Ok(section)
}

fn state_channel_section(channels: &[Value]) -> Result<Vec<u8>, String> {
    let mut strings: Vec<String> = Vec::new();
    let mut string_id = |value: &str| -> u32 {
        if let Some(index) = strings.iter().position(|held| held == value) {
            return index as u32;
        }
        strings.push(value.to_string());
        (strings.len() - 1) as u32
    };
    let mut records = Vec::new();
    for (index, channel) in channels.iter().enumerate() {
        let name = channel["name"].as_str().ok_or("channel needs a name")?.to_string();
        let kind = match channel.get("entity_kind").and_then(Value::as_str).unwrap_or("edge") {
            "node" => 1u8,
            "edge" => 2u8,
            _ => return Err(format!("channel '{name}' must apply to a node or an edge")),
        };
        let minimum = channel.get("min").and_then(Value::as_f64);
        let maximum = channel.get("max").and_then(Value::as_f64);
        if minimum.is_some() != maximum.is_some() {
            return Err(format!("channel '{name}' declares half a range"));
        }
        if let (Some(low), Some(high)) = (minimum, maximum)
            && low > high
        {
            return Err(format!("channel '{name}' declares an inverted range"));
        }
        let interpolate = channel.get("interpolate").and_then(Value::as_bool).unwrap_or(true);
        let flags = u32::from(minimum.is_some()) | if interpolate { 2 } else { 0 };
        let unit = channel.get("unit").and_then(Value::as_str).unwrap_or("1").to_string();
        let name_id = string_id(&name);
        let unit_id = string_id(&unit);
        records.extend_from_slice(&((index + 1) as u16).to_le_bytes());
        records.push(kind);
        records.push(1);
        records.extend_from_slice(&name_id.to_le_bytes());
        records.extend_from_slice(&unit_id.to_le_bytes());
        records.extend_from_slice(&(minimum.unwrap_or(0.0) as f32).to_le_bytes());
        records.extend_from_slice(&(maximum.unwrap_or(0.0) as f32).to_le_bytes());
        records.extend_from_slice(&flags.to_le_bytes());
    }
    let header_size = 12u32;
    let table = string_dictionary(&strings);
    let mut section = Vec::new();
    section.extend_from_slice(&(channels.len() as u32).to_le_bytes());
    section.extend_from_slice(&header_size.to_le_bytes());
    section.extend_from_slice(&(header_size + table.len() as u32).to_le_bytes());
    section.extend_from_slice(&table);
    section.extend_from_slice(&records);
    Ok(section)
}

fn extra_sections(extras: &[Extra]) -> Result<Vec<Vec<u8>>, String> {
    let mut ordered: Vec<&Extra> = extras.iter().collect();
    ordered.sort_by(|a, b| (&a.owner, &a.media_type).cmp(&(&b.owner, &b.media_type)));
    for pair in ordered.windows(2) {
        if (&pair[0].owner, &pair[0].media_type) == (&pair[1].owner, &pair[1].media_type) {
            return Err(format!(
                "two XTRA records share an owner and media type: '{}', '{}'",
                pair[1].owner, pair[1].media_type
            ));
        }
    }
    let mut payloads = Vec::new();
    for extra in ordered {
        if extra.owner.is_empty()
            || !extra.owner.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(format!(
                "XTRA owner must be a solver id of lowercase letters, digits and '-', got '{}'",
                extra.owner
            ));
        }
        if extra.media_type.is_empty() {
            return Err(format!("XTRA record owned by '{}' declares no media type", extra.owner));
        }
        let header_size = 28u32;
        let owner_offset = header_size;
        let media_offset = owner_offset + extra.owner.len() as u32;
        let payload_offset = media_offset + extra.media_type.len() as u32;
        let mut section = Vec::new();
        for value in [
            owner_offset,
            extra.owner.len() as u32,
            media_offset,
            extra.media_type.len() as u32,
            payload_offset,
            extra.payload.len() as u32,
            0,
        ] {
            section.extend_from_slice(&value.to_le_bytes());
        }
        section.extend_from_slice(extra.owner.as_bytes());
        section.extend_from_slice(extra.media_type.as_bytes());
        section.extend_from_slice(&extra.payload);
        payloads.push(section);
    }
    Ok(payloads)
}
