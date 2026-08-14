// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Rebuilds a SWMM `.inp` from a container.
//!
//! The network sections are written from typed properties and the graph; everything else is
//! the `XTRA` record handed back unread. Coordinates come out of quantized geometry, so they
//! are not the source numbers to the last digit — like EPANET's, this round trip is defined by
//! what a simulation reads, and SWMM's routing never reads a coordinate.

use std::collections::BTreeMap;

use serde_json::Value;
use trama_epanet::importer::Reprojection;
use trama_epanet::inp;
use trama_format::{export, parse_graph, read_sections};

use crate::importer::{MEDIA_TYPE, OWNER};

/// Sections in the order SWMM's own writer emits them, with the typed fields each row renders
/// after its name (and, for links, its endpoints). `*` renders the `swmm:parameters` tail.
const NODE_SECTIONS: [(&str, &str, &[&str]); 4] = [
    ("JUNCTIONS", "junction", &["#invert", "#max-depth", "#init-depth", "#surcharge-depth", "#ponded-area"]),
    ("OUTFALLS", "outfall", &["#invert", "*"]),
    ("STORAGE", "storage", &["#invert", "#max-depth", "#init-depth", "*"]),
    ("DIVIDERS", "divider", &["#invert", "diverted-link", "*"]),
];
const LINK_SECTIONS: [(&str, &str, &[&str]); 5] = [
    ("CONDUITS", "conduit", &["#length", "#roughness", "#in-offset", "#out-offset", "#init-flow", "#max-flow"]),
    ("PUMPS", "pump", &["*"]),
    ("ORIFICES", "orifice", &["orifice-type", "#offset", "#coeff", "gated", "#close-time"]),
    ("WEIRS", "weir", &["weir-type", "*"]),
    ("OUTLETS", "outlet", &["#offset", "*"]),
];
const XSECTION_FIELDS: [&str; 7] = ["shape", "#geom1", "#geom2", "#geom3", "#geom4", "#barrels", "culvert"];

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
        sections.push(inp::section(name, "", rows));
    }
    for (name, kind, fields) in LINK_SECTIONS {
        let rows = edges
            .iter()
            .filter(|entity| entity.kind == kind)
            .map(|entity| {
                let mut row = vec![entity.name.clone(), entity.endpoints[0].clone(), entity.endpoints[1].clone()];
                row.extend(rendered(fields, &entity.properties)?);
                Ok(row)
            })
            .collect::<Result<Vec<Vec<String>>, String>>()?;
        sections.push(inp::section(name, "", rows));
    }

    // A cross-section belongs to the link that carries it, so the section regenerates from the
    // links rather than from a store of its own.
    let xsections = edges
        .iter()
        .filter(|entity| entity.properties.get("swmm:shape").is_some())
        .map(|entity| {
            let mut row = vec![entity.name.clone()];
            row.extend(rendered(&XSECTION_FIELDS, &entity.properties)?);
            Ok(row)
        })
        .collect::<Result<Vec<Vec<String>>, String>>()?;
    sections.push(inp::section("XSECTIONS", "", xsections));

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
    document.sections.extend(remainder.sections.iter().filter(|(name, _)| name != "TITLE").cloned());
    document.sections.push(inp::section("COORDINATES", "", coordinates));
    document.sections.push(inp::section("VERTICES", "", vertices));
    Ok(inp::serialize(&document))
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
    Err("this container carries no SWMM sections; it was not compiled from a SWMM .inp".into())
}

/// Node and edge entities, each edge told which nodes it joins.
fn entities(container: &[u8]) -> Result<(Vec<Entity>, Vec<Entity>), String> {
    let sections = read_sections(container)?;
    let graph = sections.iter().find(|s| &s.kind == b"GRPH").ok_or("container is missing a GRPH section")?;
    let parsed = parse_graph(&graph.payload)?;
    let exported = export(container)?;

    let named = |feature: &Value| -> Result<Entity, String> {
        Ok(Entity {
            name: feature["properties"]["swmm:name"]
                .as_str()
                .ok_or("container was not compiled from a SWMM network")?
                .to_string(),
            kind: feature["properties"]["swmm:kind"].as_str().unwrap_or("").to_string(),
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
                feature["properties"]["swmm:name"].as_str().unwrap_or("").to_string(),
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
            by_identity.get(&id).cloned().ok_or_else(|| "node has no SWMM name".to_string())
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

/// Render the fields a row carries, up to the last one present. `*` renders the parameters
/// tail. A numeric field missing before a present one becomes `0`, which is what SWMM writes;
/// a missing text field there has no such convention, so it fails rather than guess.
fn rendered(names: &[&str], properties: &Value) -> Result<Vec<String>, String> {
    let tail = names.last() == Some(&"*");
    let typed = if tail { &names[..names.len() - 1] } else { names };
    let present: Vec<Option<&Value>> =
        typed.iter().map(|name| properties.get(format!("swmm:{}", name.trim_start_matches('#')))).collect();
    let parameters = if tail { properties.get("swmm:parameters") } else { None };
    // With a parameters tail present, every typed field before it must render.
    let last =
        if parameters.is_some() { present.len().checked_sub(1) } else { present.iter().rposition(Option::is_some) };
    let mut rendered = Vec::new();
    if let Some(last) = last {
        for (name, value) in typed[..=last].iter().zip(&present[..=last]) {
            match value {
                Some(Value::String(text)) => rendered.push(text.clone()),
                Some(number) => rendered.push(inp::text(number.as_f64().unwrap_or_default())),
                None if name.starts_with('#') => rendered.push("0".to_string()),
                None => {
                    return Err(format!(
                        "'{}' is absent but a later field is present, and SWMM has no blank for it",
                        name.trim_start_matches('#')
                    ));
                }
            }
        }
    }
    if let Some(Value::String(text)) = parameters {
        rendered.push(text.clone());
    }
    Ok(rendered)
}
