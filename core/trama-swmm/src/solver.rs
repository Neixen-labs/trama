// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Runs SWMM over a container and returns the state deltas the engine consumes.
//!
//! The container is turned back into a `.inp` and handed to the toolkit, which is the point of
//! the round trip being defined by simulation: if the export is faithful, the solver is solving
//! the user's network and not an approximation of it. The same shape as `trama-epanet`'s
//! solver, with the 5.2 API's differences: indices are zero-based, one `swmm_getValue` reads
//! every property, and elapsed time arrives in decimal days.

use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_double, c_int};
use std::path::Path;

use trama_format::{export, read_sections};
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{declared, pack};

use crate::exporter::export_inp;

/// Coordinates never reach the routing, so the projection used to rebuild the `.inp` is free.
const WORKING_CRS: &str = "EPSG:3857";

/// The toolkit's API, declared here rather than generated: bindgen against a WASI target
/// produces constants and no functions, and the browser build needs that target.
/// `trama-swmm-sys` builds and links the C.
mod toolkit {
    use std::ffi::{c_char, c_double, c_int};

    // Nothing here names a `trama_swmm_sys` item, and an unreferenced crate takes its link
    // directives with it. This keeps libswmm5 on the link line.
    use trama_swmm_sys as _;

    unsafe extern "C" {
        pub fn swmm_open(input: *const c_char, report: *const c_char, output: *const c_char) -> c_int;
        pub fn swmm_start(save_flag: c_int) -> c_int;
        pub fn swmm_stride(stride_seconds: c_int, elapsed_days: *mut c_double) -> c_int;
        pub fn swmm_end() -> c_int;
        pub fn swmm_close() -> c_int;
        pub fn swmm_getCount(object: c_int) -> c_int;
        pub fn swmm_getName(object: c_int, index: c_int, name: *mut c_char, size: c_int);
        pub fn swmm_getValue(property: c_int, index: c_int) -> c_double;
        pub fn swmm_getError(message: *mut c_char, length: c_int) -> c_int;
    }
}

const NODE: c_int = 2;
const LINK: c_int = 3;
const NODE_DEPTH: c_int = 303;
const NODE_OVERFLOW: c_int = 308;
const LINK_FLOW: c_int = 410;
const REPORT_STEP: c_int = 5; // swmm_SystemProperty::swmm_REPORTSTEP, in seconds
const SECONDS_PER_DAY: f64 = 86400.0;

pub struct SwmmSolver;

impl Solver for SwmmSolver {
    fn id(&self) -> &'static str {
        "swmm"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        let depth_channel = request.params["depth_channel"].as_str().unwrap_or("depth");
        let flow_channel = request.params["flow_channel"].as_str().unwrap_or("flow");
        let flooding_channel = request.params["flooding_channel"].as_str().unwrap_or("flooding");
        let closed = trama_solver::closed_edges(&request.params)?;
        solve(
            &request.container,
            depth_channel,
            flow_channel,
            flooding_channel,
            &closed,
            request.t0_seconds,
            request.t1_seconds,
        )
        .map_err(Rejection::input)
    }
}

/// Packed deltas for every node depth and link flow reported within [t0, t1].
///
/// `closed`: edges (by stable id) removed from the network for this run. SWMM's API refuses to
/// close a conduit (`setLinkSetting` returns on `CONDUIT`), and its `.inp` has no status
/// column for one, so a blocked link is expressed the way the engine can hear it: absent.
pub fn solve(
    container: &[u8],
    depth_channel: &str,
    flow_channel: &str,
    flooding_channel: &str,
    closed: &[u64],
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    if t1_seconds < t0_seconds {
        return Err("t1_seconds must not precede t0_seconds".into());
    }
    let depth = declared(container, depth_channel, 1)?;
    let flow = declared(container, flow_channel, 2)?;
    // Written only where the container declares it, the same rule as EPANET's age: files
    // compiled before the channel existed keep solving exactly as they did.
    let flooding = declared(container, flooding_channel, 1).ok();
    let (nodes, links) = entity_ids(container)?;

    // A counter rather than a process id: WASI has no processes, and asking for one traps.
    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let workspace = scratch().join(format!("trama-swmm-{serial}"));
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;
    let network = workspace.join("network.inp");
    let mut text = export_inp(container, WORKING_CRS)?;
    if !closed.is_empty() {
        let by_id: BTreeMap<u64, &String> = links.iter().map(|(name, id)| (*id, name)).collect();
        let mut names = std::collections::BTreeSet::new();
        for id in closed {
            names.insert(
                by_id.get(id).ok_or_else(|| format!("closed_edges names no edge of this network: {id}"))?.as_str(),
            );
        }
        // Drop the closed links from every section that rows them by name: the link sections
        // themselves, their cross-sections, and their vertices.
        let document = trama_epanet::inp::parse(&text);
        let filtered = trama_epanet::inp::Document {
            sections: document
                .sections
                .iter()
                .map(|(section, body)| {
                    let filters = ["CONDUITS", "PUMPS", "ORIFICES", "WEIRS", "OUTLETS", "XSECTIONS", "VERTICES"];
                    if !filters.contains(&section.as_str()) {
                        return (section.clone(), body.clone());
                    }
                    let kept = body
                        .iter()
                        .filter(|line| {
                            trama_epanet::inp::values(line).first().is_none_or(|name| !names.contains(name.as_str()))
                        })
                        .cloned()
                        .collect();
                    (section.clone(), kept)
                })
                .collect(),
        };
        text = trama_epanet::inp::serialize(&filtered);
    }
    std::fs::write(&network, text).map_err(|error| error.to_string())?;
    let result = simulate(&network, &workspace, &nodes, &links, depth, flow, flooding, t0_seconds, t1_seconds);
    let _ = std::fs::remove_dir_all(&workspace);
    result
}

/// `std::env::temp_dir` panics on WASI; the browser build names its preopened directory.
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

/// SWMM names mapped to stable `u64` identities, nodes first and links second.
pub type Identities = (BTreeMap<String, u64>, BTreeMap<String, u64>);

/// SWMM names mapped to stable `u64` identities, for nodes and for links. A solver writes
/// deltas against those identities; the names only exist to talk to SWMM.
pub fn entity_ids(container: &[u8]) -> Result<Identities, String> {
    let sections = read_sections(container)?;
    if !sections.iter().any(|section| &section.kind == b"XTRA") {
        return Err("container was not compiled from a SWMM network".into());
    }
    let exported = export(container)?;
    let named = |collection: &serde_json::Value| -> Result<BTreeMap<String, u64>, String> {
        collection["features"]
            .as_array()
            .ok_or("export produced no features")?
            .iter()
            .map(|feature| {
                let name = feature["properties"]["swmm:name"]
                    .as_str()
                    .ok_or("container was not compiled from a SWMM network")?;
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
    workspace: &Path,
    nodes: &BTreeMap<String, u64>,
    links: &BTreeMap<String, u64>,
    depth: u16,
    flow: u16,
    flooding: Option<u16>,
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    let path =
        |value: &Path| CString::new(value.to_string_lossy().as_bytes()).map_err(|_| "path is not usable".to_string());
    let input = path(network)?;
    let report = path(&workspace.join("report.rpt"))?;
    let output = path(&workspace.join("results.out"))?;
    let mut records = Vec::new();

    // The 5.2 API works on one global project, so two simulations cannot interleave in one
    // process. The engine's own swmm_run has the same property; the server handles one request
    // at a time, and the lock makes the library tell the truth under any caller.
    static ENGINE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _running = ENGINE.lock().map_err(|_| "a previous SWMM run panicked".to_string())?;

    unsafe {
        check(toolkit::swmm_open(input.as_ptr(), report.as_ptr(), output.as_ptr()))?;
        let outcome = (|| -> Result<(), String> {
            check(toolkit::swmm_start(0))?;
            let node_count = toolkit::swmm_getCount(NODE);
            let link_count = toolkit::swmm_getCount(LINK);
            // Values are read at every report step: stepping by the routing step instead would
            // multiply the deltas by orders of magnitude without changing what a scrub shows.
            let stride = toolkit::swmm_getValue(REPORT_STEP, 0).max(1.0) as c_int;
            loop {
                let mut elapsed_days: c_double = 0.0;
                check(toolkit::swmm_stride(stride, &mut elapsed_days))?;
                if elapsed_days <= 0.0 {
                    break;
                }
                let seconds = (elapsed_days * SECONDS_PER_DAY) as f32;
                if seconds < t0_seconds || seconds > t1_seconds {
                    continue;
                }
                for index in 0..node_count {
                    if let Some(identity) = nodes.get(&identifier(NODE, index)) {
                        let value = toolkit::swmm_getValue(NODE_DEPTH, index) as f32;
                        records.extend_from_slice(&pack(*identity, depth, seconds, value));
                        if let Some(flooding) = flooding {
                            let rate = toolkit::swmm_getValue(NODE_OVERFLOW, index) as f32;
                            records.extend_from_slice(&pack(*identity, flooding, seconds, rate));
                        }
                    }
                }
                for index in 0..link_count {
                    if let Some(identity) = links.get(&identifier(LINK, index)) {
                        let value = toolkit::swmm_getValue(LINK_FLOW, index) as f32;
                        records.extend_from_slice(&pack(*identity, flow, seconds, value));
                    }
                }
            }
            check(toolkit::swmm_end())
        })();
        toolkit::swmm_close();
        outcome?;
    }
    Ok(records)
}

fn check(code: c_int) -> Result<(), String> {
    if code == 0 {
        return Ok(());
    }
    let mut buffer = [0 as c_char; 240];
    unsafe { toolkit::swmm_getError(buffer.as_mut_ptr(), buffer.len() as c_int) };
    let bytes: Vec<u8> = buffer.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
    let message = String::from_utf8_lossy(&bytes).into_owned();
    if message.is_empty() {
        return Err(format!("SWMM refused the network: error {code}"));
    }
    Err(format!("SWMM refused the network: {message}"))
}

fn identifier(object: c_int, index: c_int) -> String {
    let mut buffer = [0 as c_char; 64];
    unsafe { toolkit::swmm_getName(object, index, buffer.as_mut_ptr(), buffer.len() as c_int) };
    let bytes: Vec<u8> = buffer.iter().take_while(|byte| **byte != 0).map(|byte| *byte as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}
