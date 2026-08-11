// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Runs EPANET over a container and returns the state deltas the engine consumes.
//!
//! The container is turned back into a `.inp` and handed to the toolkit, which is the point of
//! the round trip being defined by simulation: if the export is faithful, the solver is
//! solving the user's network and not an approximation of it.

use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_long, CString};
use std::path::Path;

use trama_format::{export, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

use crate::exporter::export_inp;

/// Coordinates never reach the hydraulics, so the projection used to rebuild the `.inp` is
/// free. Web Mercator is the one the geometry is already stored in.
const WORKING_CRS: &str = "EPSG:3857";
const PRESSURE: c_int = 11;
const FLOW: c_int = 8;
const NODE_COUNT: c_int = 0;
const LINK_COUNT: c_int = 2;
const SAVE: c_int = 1;

pub struct EpanetSolver;

impl Solver for EpanetSolver {
    fn id(&self) -> &'static str {
        "epanet"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        // Both, because nothing in the wire protocol changed between them: 0.2.0 only added a
        // way for a manifest to name more than one unit.
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        let pressure_channel = request.params["pressure_channel"].as_str().unwrap_or("pressure");
        let flow_channel = request.params["flow_channel"].as_str().unwrap_or("flow");
        solve(&request.container, pressure_channel, flow_channel, request.t0_seconds, request.t1_seconds)
            .map_err(Rejection::input)
    }
}

/// Packed deltas for every node pressure and link flow reported within [t0, t1].
pub fn solve(
    container: &[u8],
    pressure_channel: &str,
    flow_channel: &str,
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    if t1_seconds < t0_seconds {
        return Err("t1_seconds must not precede t0_seconds".into());
    }
    let pressure = declared(container, pressure_channel, 1)?;
    let flow = declared(container, flow_channel, 2)?;
    let (nodes, links) = entity_ids(container)?;

    let workspace = std::env::temp_dir().join(format!("trama-epanet-{}", std::process::id()));
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let network = workspace.join("network.inp");
    std::fs::write(&network, export_inp(container, WORKING_CRS)?).map_err(|error| error.to_string())?;
    let result = simulate(&network, &workspace.join("report.rpt"), &nodes, &links, pressure, flow, t0_seconds, t1_seconds);
    let _ = std::fs::remove_dir_all(&workspace);
    result
}

/// EPANET names mapped to stable `u64` identities, for nodes and for links. A solver writes
/// deltas against those identities; the names only exist to talk to EPANET.
pub fn entity_ids(container: &[u8]) -> Result<(BTreeMap<String, u64>, BTreeMap<String, u64>), String> {
    let sections = read_sections(container)?;
    if !sections.iter().any(|section| &section.kind == b"XTRA") {
        return Err("container was not compiled from an EPANET network".into());
    }
    let exported = export(container)?;
    let named = |collection: &serde_json::Value| -> Result<BTreeMap<String, u64>, String> {
        collection["features"]
            .as_array()
            .ok_or("export produced no features")?
            .iter()
            .map(|feature| {
                let name = feature["properties"]["epanet:name"]
                    .as_str()
                    .ok_or("container was not compiled from an EPANET network")?;
                let id: u64 = feature["properties"]["_trama_id"].as_str().unwrap_or("0").parse().unwrap_or(0);
                Ok((name.to_string(), id))
            })
            .collect()
    };
    Ok((named(&exported.nodes)?, named(&exported.edges)?))
}

#[allow(clippy::too_many_arguments)]
fn simulate(
    network: &Path,
    report: &Path,
    nodes: &BTreeMap<String, u64>,
    links: &BTreeMap<String, u64>,
    pressure: u16,
    flow: u16,
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    let path = |value: &Path| CString::new(value.to_string_lossy().as_bytes()).map_err(|_| "path is not usable".to_string());
    let network = path(network)?;
    let report = path(report)?;
    let empty = CString::new("").unwrap();
    let mut records = Vec::new();

    unsafe {
        let mut project: epanet_sys::EN_Project = std::ptr::null_mut();
        check(epanet_sys::EN_createproject(&mut project))?;
        let outcome = (|| -> Result<(), String> {
            check(epanet_sys::EN_open(project, network.as_ptr(), report.as_ptr(), empty.as_ptr()))?;
            check(epanet_sys::EN_openH(project))?;
            check(epanet_sys::EN_initH(project, SAVE))?;
            let mut node_count: c_int = 0;
            let mut link_count: c_int = 0;
            check(epanet_sys::EN_getcount(project, NODE_COUNT, &mut node_count))?;
            check(epanet_sys::EN_getcount(project, LINK_COUNT, &mut link_count))?;
            loop {
                let mut now: c_long = 0;
                check(epanet_sys::EN_runH(project, &mut now))?;
                let seconds = now as f32;
                if seconds >= t0_seconds && seconds <= t1_seconds {
                    for index in 1..=node_count {
                        if let Some(identity) = nodes.get(&identifier(project, index, true)?) {
                            records.extend_from_slice(&pack(*identity, pressure, seconds, value(project, index, PRESSURE, true)?));
                        }
                    }
                    for index in 1..=link_count {
                        if let Some(identity) = links.get(&identifier(project, index, false)?) {
                            records.extend_from_slice(&pack(*identity, flow, seconds, value(project, index, FLOW, false)?));
                        }
                    }
                }
                let mut step: c_long = 0;
                check(epanet_sys::EN_nextH(project, &mut step))?;
                if step == 0 {
                    break;
                }
            }
            check(epanet_sys::EN_closeH(project))
        })();
        epanet_sys::EN_close(project);
        epanet_sys::EN_deleteproject(project);
        outcome?;
    }
    Ok(records)
}

/// EPANET returns warnings as small positive codes; only an error stops the run.
fn check(code: c_int) -> Result<(), String> {
    if code > 6 {
        return Err(format!("EPANET refused the network: error {code}"));
    }
    Ok(())
}

unsafe fn identifier(project: epanet_sys::EN_Project, index: c_int, node: bool) -> Result<String, String> {
    let mut buffer = [0 as c_char; 32];
    let code = unsafe {
        if node {
            epanet_sys::EN_getnodeid(project, index, buffer.as_mut_ptr())
        } else {
            epanet_sys::EN_getlinkid(project, index, buffer.as_mut_ptr())
        }
    };
    check(code)?;
    let bytes: Vec<u8> = buffer.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

unsafe fn value(project: epanet_sys::EN_Project, index: c_int, property: c_int, node: bool) -> Result<f32, String> {
    let mut value = 0.0f64;
    let code = unsafe {
        if node {
            epanet_sys::EN_getnodevalue(project, index, property, &mut value)
        } else {
            epanet_sys::EN_getlinkvalue(project, index, property, &mut value)
        }
    };
    check(code)?;
    Ok(value as f32)
}
