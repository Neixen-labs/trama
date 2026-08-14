// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Runs EPANET over a container and returns the state deltas the engine consumes.
//!
//! The container is turned back into a `.inp` and handed to the toolkit, which is the point of
//! the round trip being defined by simulation: if the export is faithful, the solver is
//! solving the user's network and not an approximation of it.

use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_int, c_long};
use std::path::Path;

use trama_format::{export, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

use crate::exporter::export_inp;

/// Coordinates never reach the hydraulics, so the projection used to rebuild the `.inp` is
/// free. Web Mercator is the one the geometry is already stored in.
const WORKING_CRS: &str = "EPSG:3857";
/// The toolkit's API, declared here rather than taken from `epanet-sys`'s bindgen output.
///
/// It is fourteen stable functions of EPANET 2.3, and generating them cost more than writing
/// them: bindgen produces constants and no functions when it parses these headers for a WASI
/// target, and the browser build needs that target. `epanet-sys` still builds and links the C.
mod toolkit {
    use std::ffi::{c_char, c_int, c_long};

    // Nothing here names a `epanet_sys` item, and an unreferenced crate takes its link
    // directives with it. This keeps libepanet2 on the link line.
    use epanet_sys as _;

    pub type Project = *mut std::ffi::c_void;

    unsafe extern "C" {
        pub fn EN_createproject(project: *mut Project) -> c_int;
        pub fn EN_deleteproject(project: Project) -> c_int;
        pub fn EN_open(project: Project, input: *const c_char, report: *const c_char, output: *const c_char) -> c_int;
        pub fn EN_close(project: Project) -> c_int;
        pub fn EN_openH(project: Project) -> c_int;
        pub fn EN_initH(project: Project, flag: c_int) -> c_int;
        pub fn EN_runH(project: Project, now: *mut c_long) -> c_int;
        pub fn EN_nextH(project: Project, step: *mut c_long) -> c_int;
        pub fn EN_closeH(project: Project) -> c_int;
        pub fn EN_getcount(project: Project, object: c_int, count: *mut c_int) -> c_int;
        pub fn EN_getnodeid(project: Project, index: c_int, id: *mut c_char) -> c_int;
        pub fn EN_getlinkid(project: Project, index: c_int, id: *mut c_char) -> c_int;
        pub fn EN_getnodevalue(project: Project, index: c_int, property: c_int, value: *mut f64) -> c_int;
        pub fn EN_getlinkvalue(project: Project, index: c_int, property: c_int, value: *mut f64) -> c_int;
        pub fn EN_setqualtype(
            project: Project,
            kind: c_int,
            chemical: *const c_char,
            units: *const c_char,
            trace_node: *const c_char,
        ) -> c_int;
        pub fn EN_openQ(project: Project) -> c_int;
        pub fn EN_initQ(project: Project, flag: c_int) -> c_int;
        pub fn EN_runQ(project: Project, now: *mut c_long) -> c_int;
        pub fn EN_nextQ(project: Project, step: *mut c_long) -> c_int;
        pub fn EN_closeQ(project: Project) -> c_int;
    }
}

const PRESSURE: c_int = 11;
const FLOW: c_int = 8;
const QUALITY: c_int = 12;
/// EN_AGE: the toolkit computes each node's water age itself; no chemical involved.
const AGE_ANALYSIS: c_int = 2;
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
        let age_channel = request.params["age_channel"].as_str().unwrap_or("age");
        let closed = closed_edges(&request.params)?;
        solve(
            &request.container,
            pressure_channel,
            flow_channel,
            age_channel,
            &closed,
            request.t0_seconds,
            request.t1_seconds,
        )
        .map_err(Rejection::input)
    }
}

/// `params.closed_edges`: stable entity ids as strings, because a u64 does not survive a JSON
/// number. Shared by both EPA solvers.
pub fn closed_edges(params: &serde_json::Value) -> Result<Vec<u64>, trama_solver::server::Rejection> {
    params["closed_edges"]
        .as_array()
        .map(|values| {
            values
                .iter()
                .map(|value| {
                    value.as_str().and_then(|text| text.parse::<u64>().ok()).ok_or_else(|| {
                        trama_solver::server::Rejection::request("closed_edges holds entity ids as strings")
                    })
                })
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

/// Packed deltas for every node pressure and link flow reported within [t0, t1].
///
/// `closed`: edges (by stable id) to force `Closed` for this run — the scenario question. A
/// closed valve still exists, so it stays in the network and in the results.
pub fn solve(
    container: &[u8],
    pressure_channel: &str,
    flow_channel: &str,
    age_channel: &str,
    closed: &[u64],
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    if t1_seconds < t0_seconds {
        return Err("t1_seconds must not precede t0_seconds".into());
    }
    let pressure = declared(container, pressure_channel, 1)?;
    let flow = declared(container, flow_channel, 2)?;
    // Age is written only where the container declares it, so files compiled before the
    // channel existed keep solving exactly as they did.
    let age = declared(container, age_channel, 1).ok();
    let (nodes, links) = entity_ids(container)?;

    // A counter rather than a process id: WASI has no processes, and asking for one traps.
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let workspace = scratch().join(format!("trama-epanet-{serial}"));
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let network = workspace.join("network.inp");
    let mut text = export_inp(container, WORKING_CRS)?;
    if !closed.is_empty() {
        // EPANET's own construct for the scenario: [STATUS] closes a link at time zero without
        // removing it. Names come from the same map the deltas use, so an unknown id is a
        // caller error worth stopping on rather than a line EPANET would refuse later.
        let by_id: BTreeMap<u64, &String> = links.iter().map(|(name, id)| (*id, name)).collect();
        let mut status = String::from("[STATUS]\n");
        for id in closed {
            let name = by_id.get(id).ok_or_else(|| format!("closed_edges names no edge of this network: {id}"))?;
            status.push_str(&format!(" {name}\tClosed\n"));
        }
        let end = text.find("[END]").unwrap_or(text.len());
        text.insert_str(end, &format!("{status}\n"));
    }
    std::fs::write(&network, text).map_err(|error| error.to_string())?;
    let result =
        simulate(&network, &workspace.join("report.rpt"), &nodes, &links, pressure, flow, age, t0_seconds, t1_seconds);
    let _ = std::fs::remove_dir_all(&workspace);
    result
}

/// Where the rebuilt `.inp` is written before the toolkit opens it.
///
/// `std::env::temp_dir` panics on WASI rather than returning anything, so the browser build
/// names the directory its host preopened instead.
fn scratch() -> std::path::PathBuf {
    #[cfg(target_os = "wasi")]
    {
        std::path::PathBuf::from("/tmp")
    }
    #[cfg(not(target_os = "wasi"))]
    {
        std::env::temp_dir()
    }
}

/// EPANET names mapped to stable `u64` identities, nodes first and links second.
pub type Identities = (BTreeMap<String, u64>, BTreeMap<String, u64>);

/// EPANET names mapped to stable `u64` identities, for nodes and for links. A solver writes
/// deltas against those identities; the names only exist to talk to EPANET.
pub fn entity_ids(container: &[u8]) -> Result<Identities, String> {
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
    age: Option<u16>,
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    let path =
        |value: &Path| CString::new(value.to_string_lossy().as_bytes()).map_err(|_| "path is not usable".to_string());
    let network = path(network)?;
    let report = path(report)?;
    let empty = CString::new("").unwrap();
    let mut records = Vec::new();

    unsafe {
        let mut project: toolkit::Project = std::ptr::null_mut();
        check(toolkit::EN_createproject(&mut project))?;
        let outcome = (|| -> Result<(), String> {
            check(toolkit::EN_open(project, network.as_ptr(), report.as_ptr(), empty.as_ptr()))?;
            check(toolkit::EN_openH(project))?;
            check(toolkit::EN_initH(project, SAVE))?;
            let mut node_count: c_int = 0;
            let mut link_count: c_int = 0;
            check(toolkit::EN_getcount(project, NODE_COUNT, &mut node_count))?;
            check(toolkit::EN_getcount(project, LINK_COUNT, &mut link_count))?;
            loop {
                let mut now: c_long = 0;
                check(toolkit::EN_runH(project, &mut now))?;
                let seconds = now as f32;
                if seconds >= t0_seconds && seconds <= t1_seconds {
                    for index in 1..=node_count {
                        if let Some(identity) = nodes.get(&identifier(project, index, true)?) {
                            records.extend_from_slice(&pack(
                                *identity,
                                pressure,
                                seconds,
                                value(project, index, PRESSURE, true)?,
                            ));
                        }
                    }
                    for index in 1..=link_count {
                        if let Some(identity) = links.get(&identifier(project, index, false)?) {
                            records.extend_from_slice(&pack(
                                *identity,
                                flow,
                                seconds,
                                value(project, index, FLOW, false)?,
                            ));
                        }
                    }
                }
                let mut step: c_long = 0;
                check(toolkit::EN_nextH(project, &mut step))?;
                if step == 0 {
                    break;
                }
            }
            check(toolkit::EN_closeH(project))?;
            // The quality pass rides on the hydraulics EN_initH(SAVE) just recorded. EN_AGE is
            // switched on here so the user's .inp needs no [QUALITY] section, and EN_nextQ
            // advances by hydraulic events, which keeps age on the cadence pressure reports at.
            let Some(age) = age else { return Ok(()) };
            check(toolkit::EN_setqualtype(project, AGE_ANALYSIS, empty.as_ptr(), empty.as_ptr(), empty.as_ptr()))?;
            check(toolkit::EN_openQ(project))?;
            check(toolkit::EN_initQ(project, SAVE))?;
            loop {
                let mut now: c_long = 0;
                check(toolkit::EN_runQ(project, &mut now))?;
                let seconds = now as f32;
                if seconds >= t0_seconds && seconds <= t1_seconds {
                    for index in 1..=node_count {
                        if let Some(identity) = nodes.get(&identifier(project, index, true)?) {
                            // EN_AGE reports in hours already, matching the declared unit.
                            let hours = value(project, index, QUALITY, true)?;
                            records.extend_from_slice(&pack(*identity, age, seconds, hours));
                        }
                    }
                }
                let mut step: c_long = 0;
                check(toolkit::EN_nextQ(project, &mut step))?;
                if step == 0 {
                    break;
                }
            }
            check(toolkit::EN_closeQ(project))
        })();
        toolkit::EN_close(project);
        toolkit::EN_deleteproject(project);
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

unsafe fn identifier(project: toolkit::Project, index: c_int, node: bool) -> Result<String, String> {
    let mut buffer = [0 as c_char; 32];
    let code = unsafe {
        if node {
            toolkit::EN_getnodeid(project, index, buffer.as_mut_ptr())
        } else {
            toolkit::EN_getlinkid(project, index, buffer.as_mut_ptr())
        }
    };
    check(code)?;
    let bytes: Vec<u8> = buffer.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

unsafe fn value(project: toolkit::Project, index: c_int, property: c_int, node: bool) -> Result<f32, String> {
    let mut value = 0.0f64;
    let code = unsafe {
        if node {
            toolkit::EN_getnodevalue(project, index, property, &mut value)
        } else {
            toolkit::EN_getlinkvalue(project, index, property, &mut value)
        }
    };
    check(code)?;
    Ok(value as f32)
}
