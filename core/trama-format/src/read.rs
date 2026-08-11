// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reading a container back, with the checks a reader must not skip (SPEC 2.2, 7).

use std::collections::BTreeSet;

const MAGIC: &[u8; 8] = b"TRAMA\0\0\0";
const HEADER_BYTES: usize = 64;
const DIRECTORY_BYTES: usize = 64;
const EXTRA_HEADER_BYTES: u32 = 28;

pub struct Section {
    pub kind: [u8; 4],
    pub key: (u32, u32, u32),
    pub payload: Vec<u8>,
}

pub struct Node {
    pub id: u64,
    pub property_row: u32,
}

pub struct Edge {
    pub id: u64,
    pub source: u32,
    pub target: u32,
    pub property_row: u32,
    pub reference_start: u32,
    pub reference_count: u32,
    /// SPEC 4 `Edge.flags` bit 0: the edge is traversable from its source only.
    pub directed: bool,
}

pub struct GeometryReference {
    pub directory_index: u32,
    pub path_index: u32,
    pub direction: i8,
}

pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub references: Vec<GeometryReference>,
    /// SPEC 4: `adjacency[csr_offsets[n]..csr_offsets[n + 1]]` is what leaves node `n`.
    pub csr_offsets: Vec<u64>,
    pub adjacency: Vec<Adjacency>,
}

pub struct Adjacency {
    pub edge_index: u32,
    /// `+1` source to target, `-1` target to source.
    pub traversal_direction: i8,
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().unwrap())
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().unwrap())
}

/// Verify a container and return every section as (kind, tile key, decoded payload).
pub fn read_sections(data: &[u8]) -> Result<Vec<Section>, String> {
    if data.len() < HEADER_BYTES {
        return Err("container is shorter than its header".into());
    }
    let header_bytes = u32_at(data, 0x14);
    let directory_offset = u64_at(data, 0x18);
    let section_count = u32_at(data, 0x20) as usize;
    let file_bytes = u64_at(data, 0x28);
    if &data[..8] != MAGIC
        || header_bytes as usize != HEADER_BYTES
        || directory_offset as usize != HEADER_BYTES
        || file_bytes as usize != data.len()
    {
        return Err("invalid container header".into());
    }
    let directory_end = directory_offset as usize + section_count * DIRECTORY_BYTES;
    if directory_end > data.len() {
        return Err("container directory exceeds file size".into());
    }

    let mut sections = Vec::with_capacity(section_count);
    let mut owners: BTreeSet<(String, String)> = BTreeSet::new();
    for index in 0..section_count {
        let record = directory_offset as usize + index * DIRECTORY_BYTES;
        let kind: [u8; 4] = data[record..record + 4].try_into().unwrap();
        let flags = u32_at(data, record + 4);
        let key = (u32_at(data, record + 8), u32_at(data, record + 12), u32_at(data, record + 16));
        let offset = u64_at(data, record + 20) as usize;
        let stored_bytes = u64_at(data, record + 28) as usize;
        let decoded_bytes = u64_at(data, record + 36) as usize;
        let checksum = u32_at(data, record + 44);
        let codec = u16_at(data, record + 48);
        if codec != 1 || offset < directory_end || offset + stored_bytes > data.len() {
            return Err("invalid section record".into());
        }
        let decoded = zstd::bulk::decompress(&data[offset..offset + stored_bytes], decoded_bytes)
            .map_err(|_| "invalid zstd section".to_string())?;
        if decoded.len() != decoded_bytes || crate::crc32c(&decoded) != checksum {
            return Err("invalid section integrity".into());
        }
        if &kind == b"XTRA" {
            validate_extra(&decoded, flags, key, &mut owners)?;
        }
        sections.push(Section { kind, key, payload: decoded });
    }
    Ok(sections)
}

/// Check what a reader relies on but cannot recover if a writer got it wrong (SPEC 7).
fn validate_extra(
    payload: &[u8],
    flags: u32,
    key: (u32, u32, u32),
    seen: &mut BTreeSet<(String, String)>,
) -> Result<(), String> {
    if flags & 1 != 0 {
        return Err("an XTRA record must be optional, so an older reader can skip it".into());
    }
    if key != (0, 0, 0) {
        return Err("an XTRA record is not tile-scoped, so its tile key must be zero".into());
    }
    if payload.len() < EXTRA_HEADER_BYTES as usize {
        return Err("XTRA record is shorter than its header".into());
    }
    let spans = [
        (u32_at(payload, 0), u32_at(payload, 4)),
        (u32_at(payload, 8), u32_at(payload, 12)),
        (u32_at(payload, 16), u32_at(payload, 20)),
    ];
    let extra_flags = u32_at(payload, 24);
    let out_of_bounds = spans
        .iter()
        .any(|(offset, length)| *offset < EXTRA_HEADER_BYTES || (*offset + *length) as usize > payload.len());
    if extra_flags != 0 || out_of_bounds {
        return Err("invalid XTRA record header".into());
    }
    let text = |(offset, length): (u32, u32)| {
        String::from_utf8_lossy(&payload[offset as usize..(offset + length) as usize]).into_owned()
    };
    let identity = (text(spans[0]), text(spans[1]));
    if !seen.insert(identity.clone()) {
        return Err(format!("two XTRA records share an owner and media type: '{}', '{}'", identity.0, identity.1));
    }
    Ok(())
}

/// Ascending ids from a block of unsigned LEB128 gaps (SPEC 4.1).
fn identities(payload: &[u8], offset: usize, count: usize) -> Result<Vec<u64>, String> {
    let mut identities = Vec::with_capacity(count);
    let mut value = 0u64;
    let mut at = offset;
    for _ in 0..count {
        let mut gap = 0u64;
        let mut shift = 0u32;
        loop {
            if at >= payload.len() {
                return Err("identity block runs past the section".into());
            }
            let group = payload[at];
            at += 1;
            gap |= u64::from(group & 0x7F) << shift;
            shift += 7;
            if group & 0x80 == 0 {
                break;
            }
        }
        value = value.wrapping_add(gap);
        identities.push(value);
    }
    Ok(identities)
}

/// Decode a GRPH payload into its nodes, edges, and geometry references.
pub fn parse_graph(payload: &[u8]) -> Result<Graph, String> {
    if payload.len() < 44 {
        return Err("graph section is shorter than its header".into());
    }
    let node_count = u32_at(payload, 0) as usize;
    let edge_count = u32_at(payload, 4) as usize;
    let adjacency_count = u32_at(payload, 8) as usize;
    let reference_count = u32_at(payload, 12) as usize;
    let nodes_offset = u32_at(payload, 16) as usize;
    let edges_offset = u32_at(payload, 20) as usize;
    let csr_offset = u32_at(payload, 24) as usize;
    let adjacency_offset = u32_at(payload, 28) as usize;
    let references_offset = u32_at(payload, 32) as usize;
    let node_ids_offset = u32_at(payload, 36) as usize;
    let edge_ids_offset = u32_at(payload, 40) as usize;
    let bound = |offset: usize, count: usize, stride: usize| -> Result<(), String> {
        if offset + count * stride > payload.len() {
            return Err("graph section array runs past the section".into());
        }
        Ok(())
    };
    bound(nodes_offset, node_count, 8)?;
    bound(edges_offset, edge_count, 24)?;
    bound(csr_offset, node_count + 1, 8)?;
    bound(adjacency_offset, adjacency_count, 8)?;
    bound(references_offset, reference_count, 12)?;

    let node_ids = identities(payload, node_ids_offset, node_count)?;
    let edge_ids = identities(payload, edge_ids_offset, edge_count)?;
    let nodes = (0..node_count)
        .map(|index| Node { id: node_ids[index], property_row: u32_at(payload, nodes_offset + index * 8) })
        .collect();
    let edges = (0..edge_count)
        .map(|index| {
            let at = edges_offset + index * 24;
            Edge {
                id: edge_ids[index],
                source: u32_at(payload, at),
                target: u32_at(payload, at + 4),
                property_row: u32_at(payload, at + 8),
                reference_start: u32_at(payload, at + 12),
                reference_count: u32_at(payload, at + 16),
                directed: u32_at(payload, at + 20) & 1 != 0,
            }
        })
        .collect();
    let references = (0..reference_count)
        .map(|index| {
            let at = references_offset + index * 12;
            GeometryReference {
                directory_index: u32_at(payload, at),
                path_index: u32_at(payload, at + 4),
                direction: payload[at + 8] as i8,
            }
        })
        .collect();
    let csr_offsets: Vec<u64> = (0..=node_count).map(|index| u64_at(payload, csr_offset + index * 8)).collect();
    // SPEC 4: the bounds a reader must check before trusting a slice of the adjacency array.
    if csr_offsets[0] != 0 || csr_offsets[node_count] != adjacency_count as u64 {
        return Err("CSR offsets must run from zero to the adjacency count".into());
    }
    if csr_offsets.windows(2).any(|pair| pair[1] < pair[0]) {
        return Err("CSR offsets must be monotonic".into());
    }
    let adjacency = (0..adjacency_count)
        .map(|index| {
            let at = adjacency_offset + index * 8;
            Adjacency { edge_index: u32_at(payload, at), traversal_direction: payload[at + 4] as i8 }
        })
        .collect();
    Ok(Graph { nodes, edges, references, csr_offsets, adjacency })
}
