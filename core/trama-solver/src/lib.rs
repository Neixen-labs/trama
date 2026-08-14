// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! What every solver shares: the packed delta, channel resolution, and the server runtime.
//!
//! In Python these lived twice, once per solver, on the argument that two implementations of
//! a protocol are not yet a pattern. Rewriting both in one language is the moment that stops
//! being true, so the framing lives here and each solver brings only its own arithmetic.

pub mod server;

use trama_format::read_sections;

/// SPEC 4 of the solver contract: `(u64 entity_id, u16 channel, f32 t, f32 value)`, no padding.
pub const DELTA_BYTES: usize = 18;

pub fn pack(entity_id: u64, channel: u16, t: f32, value: f32) -> [u8; DELTA_BYTES] {
    let mut record = [0u8; DELTA_BYTES];
    record[0..8].copy_from_slice(&entity_id.to_le_bytes());
    record[8..10].copy_from_slice(&channel.to_le_bytes());
    record[10..14].copy_from_slice(&t.to_le_bytes());
    record[14..18].copy_from_slice(&value.to_le_bytes());
    record
}

pub struct Channel {
    pub id: u16,
    pub entity_kind: u8,
    pub name: String,
    pub unit: String,
    pub declared_min: f32,
    pub declared_max: f32,
    pub range_present: bool,
}

/// Read STCH. The file declares what solvers may write; it holds no samples.
pub fn channels(container: &[u8]) -> Result<Vec<Channel>, String> {
    let sections = read_sections(container)?;
    let payload = &sections
        .iter()
        .find(|section| &section.kind == b"STCH")
        .ok_or("container is missing an STCH section")?
        .payload;
    let u32_at = |at: usize| u32::from_le_bytes(payload[at..at + 4].try_into().unwrap());
    let count = u32_at(0) as usize;
    let strings_offset = u32_at(4) as usize;
    let records_offset = u32_at(8) as usize;

    let mut strings = Vec::new();
    let mut at = strings_offset + 4;
    for _ in 0..u32_at(strings_offset) as usize {
        let length = u32_at(at) as usize;
        strings.push(String::from_utf8_lossy(&payload[at + 4..at + 4 + length]).into_owned());
        at += 4 + length;
    }
    let text = |index: usize| strings.get(index).cloned().unwrap_or_default();

    (0..count)
        .map(|index| {
            let at = records_offset + index * 24;
            if at + 24 > payload.len() {
                return Err("state channel runs past the section".into());
            }
            let flags = u32_at(at + 20);
            Ok(Channel {
                id: u16::from_le_bytes(payload[at..at + 2].try_into().unwrap()),
                entity_kind: payload[at + 2],
                name: text(u32_at(at + 4) as usize),
                unit: text(u32_at(at + 8) as usize),
                declared_min: f32::from_le_bytes(payload[at + 12..at + 16].try_into().unwrap()),
                declared_max: f32::from_le_bytes(payload[at + 16..at + 20].try_into().unwrap()),
                range_present: flags & 1 != 0,
            })
        })
        .collect()
}

/// Resolve a channel name to its declared id, refusing one the file never promised.
pub fn declared(container: &[u8], name: &str, entity_kind: u8) -> Result<u16, String> {
    channels(container)?
        .into_iter()
        .find(|channel| channel.name == name && channel.entity_kind == entity_kind)
        .map(|channel| channel.id)
        .ok_or_else(|| {
            let kind = if entity_kind == 1 { "node" } else { "edge" };
            format!("the container declares no {kind} channel named '{name}'")
        })
}

/// Checks a `solver.toml` against the implementation it claims to describe.
///
/// A manifest is a promise made in a file nothing compiles, so it rots quietly: the id changes in
/// code, the manifest keeps the old one, and a registry resolves a solver that is not there. This
/// is the smallest guard against that, and it belongs beside the contract rather than in one
/// crate's tests, since every implementor needs the same check.
///
/// ponytail: reads `key = value` lines above the first table header, which is all the fields it
/// compares. It is not a TOML parser and will not notice a malformed manifest that happens to
/// spell those two keys correctly — `id` and `contract_versions` are what drift.
pub fn manifest_agrees_with(manifest: &str, id: &str, contract_versions: &[&str]) -> Result<(), String> {
    let declared = |key: &str| {
        manifest
            .lines()
            .take_while(|line| !line.trim_start().starts_with('['))
            .filter_map(|line| line.split_once('='))
            .find(|(name, _)| name.trim() == key)
            .map(|(_, value)| value.trim().to_string())
    };
    let quoted = |value: &str| value.trim().trim_matches('"').to_string();

    match declared("id") {
        Some(value) if quoted(&value) == id => {}
        Some(value) => return Err(format!("manifest id {value} is not the solver's '{id}'")),
        None => return Err("manifest declares no id".into()),
    }
    let versions = declared("contract_versions").ok_or("manifest declares no contract_versions")?;
    let listed: Vec<String> =
        versions.trim_matches(['[', ']']).split(',').map(quoted).filter(|value| !value.is_empty()).collect();
    if listed != contract_versions {
        return Err(format!("manifest contract_versions {listed:?} are not the solver's {contract_versions:?}"));
    }
    Ok(())
}

/// `params.closed_edges`: stable entity ids as strings, because a u64 does not survive a JSON
/// number. A contract-level convention rather than one solver's: any solver simulating a
/// network can be asked to run it with edges closed.
pub fn closed_edges(params: &serde_json::Value) -> Result<Vec<u64>, server::Rejection> {
    params["closed_edges"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value
                        .as_str()
                        .and_then(|text| text.parse::<u64>().ok())
                        .ok_or_else(|| server::Rejection::request("closed_edges holds entity ids as strings"))
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}
