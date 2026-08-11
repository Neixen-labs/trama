// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Rebuilds a `.inp` from a container.
//!
//! The entity sections are written from typed properties and the graph; everything else is the
//! `XTRA` record handed back unread. Coordinates come out of quantized geometry, so they are
//! not the source numbers to the last digit — SPEC 9 defines this round trip by simulation
//! results, and EPANET's hydraulics never read a coordinate.

use std::collections::BTreeMap;

use serde_json::Value;
use trama_format::{export, parse_graph, read_sections};

use crate::importer::{MEDIA_TYPE, OWNER, Reprojection};
use crate::inp;

const HEADERS: [(&str, &str); 8] = [
    ("JUNCTIONS", "ID              \tElev        \tDemand      \tPattern"),
    ("RESERVOIRS", "ID              \tHead        \tPattern"),
    (
        "TANKS",
        "ID              \tElevation   \tInitLevel   \tMinLevel    \tMaxLevel    \tDiameter    \tMinVol      \tVolCurve",
    ),
    (
        "PIPES",
        "ID              \tNode1           \tNode2           \tLength      \tDiameter    \tRoughness   \tMinorLoss   \tStatus",
    ),
    ("PUMPS", "ID              \tNode1           \tNode2           \tParameters"),
    ("VALVES", "ID              \tNode1           \tNode2           \tDiameter    \tType\tSetting     \tMinorLoss"),
    ("COORDINATES", "Node            \tX-Coord         \tY-Coord"),
    ("VERTICES", "Link            \tX-Coord         \tY-Coord"),
];
const NODE_SECTIONS: [(&str, &str, &[&str]); 3] = [
    ("JUNCTIONS", "junction", &["#elevation", "#demand", "pattern"]),
    ("RESERVOIRS", "reservoir", &["#head", "pattern"]),
    (
        "TANKS",
        "tank",
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

struct Entity {
    name: String,
    kind: String,
    properties: Value,
    coordinates: Vec<(f64, f64)>,
    endpoints: [String; 2],
}

/// Write a `.inp`, reprojecting geometry back into `crs`, the one the import was given.
pub fn export_inp(container: &[u8], crs: &str) -> Result<String, String> {
    let remainder = remainder(container)?;
    let (nodes, edges) = entities(container)?;
    let back = Reprojection::from_wgs84(crs)?;

    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    for (name, kind, fields) in NODE_SECTIONS {
        let rows = nodes
            .iter()
            .filter(|entity| entity.kind == kind)
            .map(|entity| {
                let mut row = vec![entity.name.clone()];
                row.extend(rendered(fields, &entity.properties)?);
                Ok(row)
            })
            .collect::<Result<Vec<Vec<String>>, String>>()?;
        sections.push(inp::section(name, header(name), rows));
    }
    for (name, kind, fields) in [
        ("PIPES", "pipe", &["#length", "#diameter", "#roughness", "#minor-loss", "status"][..]),
        ("PUMPS", "pump", &["epanet:parameters"][..]),
        ("VALVES", "valve", &["#diameter", "valve-type", "setting", "#minor-loss"][..]),
    ] {
        let rows = edges
            .iter()
            .filter(|entity| entity.kind == kind)
            .map(|entity| {
                let mut row = vec![entity.name.clone(), entity.endpoints[0].clone(), entity.endpoints[1].clone()];
                if kind == "pump" {
                    row.push(entity.properties["epanet:parameters"].as_str().unwrap_or("").to_string());
                } else {
                    row.extend(rendered(fields, &entity.properties)?);
                }
                Ok(row)
            })
            .collect::<Result<Vec<Vec<String>>, String>>()?;
        sections.push(inp::section(name, header(name), rows));
    }

    let coordinates = nodes
        .iter()
        .map(|entity| {
            let (x, y) = back.apply(entity.coordinates[0])?;
            Ok(vec![entity.name.clone(), format!("{x:.4}"), format!("{y:.4}")])
        })
        .collect::<Result<Vec<Vec<String>>, String>>()?;
    let mut vertices = Vec::new();
    for entity in &edges {
        for point in &entity.coordinates[1..entity.coordinates.len().saturating_sub(1)] {
            let (x, y) = back.apply(*point)?;
            vertices.push(vec![entity.name.clone(), format!("{x:.4}"), format!("{y:.4}")]);
        }
    }

    fn take(document: &inp::Document, wanted: &str) -> Vec<(String, Vec<String>)> {
        document.sections.iter().filter(|(name, _body)| name == wanted).cloned().collect()
    }
    let mut document = inp::Document { sections: take(&remainder, "TITLE") };
    document.sections.extend(sections);
    document.sections.extend(remainder.sections.iter().filter(|(name, _)| name != "TITLE" && name != "END").cloned());
    document.sections.push(inp::section("COORDINATES", header("COORDINATES"), coordinates));
    document.sections.push(inp::section("VERTICES", header("VERTICES"), vertices));
    let end = take(&remainder, "END");
    document.sections.extend(if end.is_empty() { vec![("END".to_string(), Vec::new())] } else { end });
    Ok(inp::serialize(&document))
}

fn header(name: &str) -> &'static str {
    HEADERS.iter().find(|(section, _)| *section == name).map(|(_, text)| *text).unwrap_or("")
}

/// The sections the core carried without reading them.
fn remainder(container: &[u8]) -> Result<inp::Document, String> {
    for section in read_sections(container)? {
        if &section.kind != b"XTRA" {
            continue;
        }
        let at = |offset: usize| u32::from_le_bytes(section.payload[offset..offset + 4].try_into().unwrap()) as usize;
        let owner = String::from_utf8_lossy(&section.payload[at(0)..at(0) + at(4)]).into_owned();
        let media_type = String::from_utf8_lossy(&section.payload[at(8)..at(8) + at(12)]).into_owned();
        if owner == OWNER && media_type == MEDIA_TYPE {
            return Ok(inp::parse(&String::from_utf8_lossy(&section.payload[at(16)..at(16) + at(20)])));
        }
    }
    Err("this container carries no EPANET sections; it was not compiled from a .inp".into())
}

/// Node and edge entities, each edge told which nodes it joins.
fn entities(container: &[u8]) -> Result<(Vec<Entity>, Vec<Entity>), String> {
    let sections = read_sections(container)?;
    let graph = sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?;
    let parsed = parse_graph(&graph.payload)?;
    let exported = export(container)?;

    let named = |feature: &Value| -> Result<Entity, String> {
        Ok(Entity {
            name: feature["properties"]["epanet:name"]
                .as_str()
                .ok_or("container was not compiled from an EPANET network")?
                .to_string(),
            kind: feature["properties"]["epanet:kind"].as_str().unwrap_or("").to_string(),
            properties: feature["properties"].clone(),
            coordinates: coordinates_of(feature),
            endpoints: [String::new(), String::new()],
        })
    };
    let mut nodes: Vec<Entity> =
        exported.nodes["features"].as_array().unwrap().iter().map(named).collect::<Result<_, String>>()?;
    let by_identity: BTreeMap<String, String> = exported.nodes["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|feature| {
            (
                feature["properties"]["_trama_id"].as_str().unwrap_or("").to_string(),
                feature["properties"]["epanet:name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    let identity_of: BTreeMap<String, (u32, u32)> =
        parsed.edges.iter().map(|edge| (edge.id.to_string(), (edge.source, edge.target))).collect();

    let mut edges: Vec<Entity> = Vec::new();
    for feature in exported.edges["features"].as_array().unwrap() {
        let mut entity = named(feature)?;
        let identity = feature["properties"]["_trama_id"].as_str().unwrap_or("");
        let (source, target) = identity_of.get(identity).ok_or("edge is not in the graph")?;
        let name_of = |index: u32| -> Result<String, String> {
            let id = parsed.nodes.get(index as usize).ok_or("edge names a missing node")?.id.to_string();
            by_identity.get(&id).cloned().ok_or_else(|| "node has no EPANET name".to_string())
        };
        entity.endpoints = [name_of(*source)?, name_of(*target)?];
        edges.push(entity);
    }
    nodes.sort_by(|a, b| a.name.cmp(&b.name));
    edges.sort_by(|a, b| a.name.cmp(&b.name));
    Ok((nodes, edges))
}

fn coordinates_of(feature: &Value) -> Vec<(f64, f64)> {
    match &feature["geometry"]["type"] {
        Value::String(kind) if kind == "Point" => {
            let pair = feature["geometry"]["coordinates"].as_array().unwrap();
            vec![(pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap())]
        }
        _ => feature["geometry"]["coordinates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|pair| (pair[0].as_f64().unwrap(), pair[1].as_f64().unwrap()))
            .collect(),
    }
}

/// Render the fields a row carries, up to the last one present.
///
/// A numeric field missing before a present one becomes `0`, which is what EPANET writes. A
/// missing text field in that position has no such convention, so it fails rather than guess
/// at a placeholder EPANET might read as a pattern name.
fn rendered(names: &[&str], properties: &Value) -> Result<Vec<String>, String> {
    let present: Vec<Option<&Value>> =
        names.iter().map(|name| properties.get(format!("epanet:{}", name.trim_start_matches('#')))).collect();
    let last = present.iter().rposition(Option::is_some);
    let Some(last) = last else { return Ok(Vec::new()) };
    let mut rendered = Vec::with_capacity(last + 1);
    for (name, value) in names[..=last].iter().zip(&present[..=last]) {
        match value {
            Some(Value::String(text)) => rendered.push(text.clone()),
            Some(number) => rendered.push(inp::text(number.as_f64().unwrap_or_default())),
            None if name.starts_with('#') => rendered.push("0".to_string()),
            None => {
                return Err(format!(
                    "'{}' is absent but a later field is present, and EPANET has no blank for it",
                    name.trim_start_matches('#')
                ));
            }
        }
    }
    Ok(rendered)
}
