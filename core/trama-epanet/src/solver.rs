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
/// EN_TRACE: the share of a node's water, in percent, that arrived from one named source.
const TRACE_ANALYSIS: c_int = 3;
const NODE_COUNT: c_int = 0;
const LINK_COUNT: c_int = 2;
const SAVE: c_int = 1;

pub struct EpanetSolver;

impl Solver for EpanetSolver {
    fn id(&self) -> &'static str {
        "epanet"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        // All three, because nothing in the wire protocol changed between them: 0.2.0 added a
        // way for a manifest to name more than one unit, 0.3.0 a way to name a channel family
        // by prefix.
        &["0.1.0", "0.2.0", "0.3.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        let pressure_channel = request.params["pressure_channel"].as_str().unwrap_or("pressure");
        let flow_channel = request.params["flow_channel"].as_str().unwrap_or("flow");
        let age_channel = request.params["age_channel"].as_str().unwrap_or("age");
        let closed = trama_solver::closed_edges(&request.params)?;
        let mut deltas = solve(
            &request.container,
            pressure_channel,
            flow_channel,
            age_channel,
            &closed,
            request.t0_seconds,
            request.t1_seconds,
        )
        .map_err(Rejection::input)?;
        // The fire-flow study rides on the same request when asked for, so one call answers both
        // "what is the network doing" and "what could it give here" — and answers the second
        // under the same closures as the first.
        let fire_nodes = trama_solver::entity_list(&request.params, "fire_nodes")?;
        let residual = request.params["residual_pressure"].as_f64().map(|value| value as f32);
        deltas.extend(
            fire_flow(
                &request.container,
                request.params["fire_flow_channel"].as_str().unwrap_or("fire_flow"),
                &fire_nodes,
                residual,
                &closed,
                request.t0_seconds,
            )
            .map_err(Rejection::input)?,
        );
        Ok(deltas)
    }
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
    let text = network_text(container, &links, closed)?;
    // The quality channels the file itself names, resolved from the same text the toolkit is
    // about to read. Each is an offer like age: undeclared means unwritten, never an error.
    let document = crate::inp::parse(&text);
    let chemical = document
        .rows("OPTIONS")
        .into_iter()
        .find(|row| row[0].eq_ignore_ascii_case("quality"))
        .and_then(|row| row.get(1).cloned())
        .filter(|name| !["none", "age", "trace"].contains(&name.to_lowercase().as_str()))
        .and_then(|name| declared(container, &format!("chem:{}", name.to_lowercase()), 1).ok());
    let traces: Vec<(String, u16)> = document
        .rows("RESERVOIRS")
        .into_iter()
        .filter_map(|row| declared(container, &format!("trace:{}", row[0]), 1).ok().map(|id| (row[0].clone(), id)))
        .collect();
    let quality = Quality { chemical, traces, age };
    std::fs::write(&network, text).map_err(|error| error.to_string())?;
    let result = simulate(
        &network,
        &workspace.join("report.rpt"),
        &nodes,
        &links,
        pressure,
        flow,
        &quality,
        t0_seconds,
        t1_seconds,
    );
    let _ = std::fs::remove_dir_all(&workspace);
    result
}

/// The quality analyses one run writes, each already resolved against the container's
/// declarations. All ride the same saved hydraulics; they differ only in what EPANET is asked
/// to carry through the pipes.
struct Quality {
    chemical: Option<u16>,
    traces: Vec<(String, u16)>,
    age: Option<u16>,
}

/// The residual pressure a fire flow must leave standing, when the caller names none.
///
/// Twenty psi is the figure fire codes and insurers work from. The metric equivalent is what
/// the same requirement is in metres of head, not a rounder number chosen for looking tidy.
const DEFAULT_RESIDUAL_PSI: f32 = 20.0;
const DEFAULT_RESIDUAL_METRES: f32 = 14.06;
/// Bisection stops when the bracket is within a thousandth of the flow, or after this many
/// halvings — whichever comes first. Each step is a full hydraulic solve.
const BISECTIONS: u32 = 24;
const BRACKET_TOLERANCE: f32 = 1e-3;

/// The largest demand each named node can be drawn at while the network holds `residual`.
///
/// This is the one question the other channels cannot answer, and the one operation here that is
/// not a reading of a single simulation: it is a search over many. A hydrant's rating is not a
/// property of the pipe it hangs off, it is a property of the whole network on the day, which is
/// why a utility has to model it and why fire departments and insurers ask for it in writing.
///
/// Written only where the container declares the channel, like every other offer, and composable
/// with `closed`: "what can this hydrant deliver with that valve shut" is what a real study asks.
pub fn fire_flow(
    container: &[u8],
    fire_channel: &str,
    fire_nodes: &[u64],
    residual: Option<f32>,
    closed: &[u64],
    t0_seconds: f32,
) -> Result<Vec<u8>, String> {
    let Ok(channel) = declared(container, fire_channel, 1) else { return Ok(Vec::new()) };
    if fire_nodes.is_empty() {
        return Ok(Vec::new());
    }
    let (nodes, links) = entity_ids(container)?;
    let by_id: BTreeMap<u64, &String> = nodes.iter().map(|(name, id)| (*id, name)).collect();
    let text = network_text(container, &links, closed)?;
    let threshold =
        residual.unwrap_or_else(|| if is_us_units(&text) { DEFAULT_RESIDUAL_PSI } else { DEFAULT_RESIDUAL_METRES });

    static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let workspace = scratch().join(format!("trama-epanet-fire-{serial}"));
    std::fs::create_dir_all(&workspace).map_err(|error| error.to_string())?;

    let mut records = Vec::new();
    let outcome = (|| -> Result<(), String> {
        for id in fire_nodes {
            let name = by_id.get(id).ok_or_else(|| format!("fire_nodes names no node of this network: {id}"))?;
            let available = search(&workspace, &text, name, threshold, t0_seconds)?;
            records.extend_from_slice(&pack(*id, channel, t0_seconds, available));
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&workspace);
    outcome?;
    Ok(records)
}

/// `[OPTIONS] Units` decides psi against metres, exactly as the importer's declaration does.
fn is_us_units(text: &str) -> bool {
    crate::inp::parse(text)
        .rows("OPTIONS")
        .into_iter()
        .find(|row| row[0].eq_ignore_ascii_case("units"))
        .and_then(|row| row.last().map(|value| value.to_lowercase()))
        .map(|units| crate::importer::US_FLOW_UNITS.contains(&units.as_str()))
        .unwrap_or(true)
}

/// Bracket upward, then bisect: the largest added demand this node still sustains.
fn search(workspace: &Path, text: &str, node: &str, threshold: f32, t0_seconds: f32) -> Result<f32, String> {
    // A node already below the threshold with no fire drawn has nothing to give, and reporting
    // some positive figure for it would be the most dangerous kind of wrong answer.
    if pressure_with(workspace, text, node, 0.0, t0_seconds)? < threshold {
        return Ok(0.0);
    }
    // Doubling finds the bracket in a logarithmic number of solves without the caller having to
    // guess a network's scale. The ceiling is a runaway guard, not a modelling limit.
    let mut low = 0.0f32;
    let mut high = 10.0f32;
    for _ in 0..BISECTIONS {
        if pressure_with(workspace, text, node, high, t0_seconds)? < threshold {
            break;
        }
        low = high;
        high *= 2.0;
    }
    if pressure_with(workspace, text, node, high, t0_seconds)? >= threshold {
        return Ok(high);
    }
    for _ in 0..BISECTIONS {
        if high - low <= BRACKET_TOLERANCE * high.max(1.0) {
            break;
        }
        let middle = 0.5 * (low + high);
        if pressure_with(workspace, text, node, middle, t0_seconds)? >= threshold {
            low = middle;
        } else {
            high = middle;
        }
    }
    Ok(low)
}

/// The pressure at `node` once `demand` is drawn there on top of whatever the file already asks.
///
/// The extra demand goes in a `[DEMANDS]` section appended to the rebuilt network, which EPANET
/// adds to the junction's own — the same export-then-simulate seam the closures use, so no
/// toolkit call is needed to change what the model is being asked for.
fn pressure_with(workspace: &Path, text: &str, node: &str, demand: f32, t0_seconds: f32) -> Result<f32, String> {
    let mut network = text.to_string();
    if demand > 0.0 {
        let end = network.find("[END]").unwrap_or(network.len());
        network.insert_str(end, &format!("[DEMANDS]\n {node}\t{demand}\t\t;fire\n\n"));
    }
    let path = workspace.join("fire.inp");
    std::fs::write(&path, network).map_err(|error| error.to_string())?;
    // A network that will not converge under this draw is a network that cannot supply it, which
    // is an answer rather than a failure: the search treats it as pressure below the threshold.
    Ok(steady_pressure(&path, &workspace.join("fire.rpt"), node, t0_seconds).unwrap_or(f32::MIN))
}

/// The network as text, with any scenario closures already in it.
///
/// `[STATUS]` is EPANET's own construct for the scenario: it closes a link at time zero without
/// removing it. Names come from the same map the deltas use, so an unknown id is a caller error
/// worth stopping on rather than a line EPANET would refuse later.
fn network_text(container: &[u8], links: &BTreeMap<String, u64>, closed: &[u64]) -> Result<String, String> {
    let mut text = export_inp(container, WORKING_CRS)?;
    if closed.is_empty() {
        return Ok(text);
    }
    let by_id: BTreeMap<u64, &String> = links.iter().map(|(name, id)| (*id, name)).collect();
    let mut status = String::from("[STATUS]\n");
    for id in closed {
        let name = by_id.get(id).ok_or_else(|| format!("closed_edges names no edge of this network: {id}"))?;
        status.push_str(&format!(" {name}\tClosed\n"));
    }
    let end = text.find("[END]").unwrap_or(text.len());
    text.insert_str(end, &format!("{status}\n"));
    Ok(text)
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
    quality: &Quality,
    t0_seconds: f32,
    t1_seconds: f32,
) -> Result<Vec<u8>, String> {
    let path =
        |value: &Path| CString::new(value.to_string_lossy().as_bytes()).map_err(|_| "path is not usable".to_string());
    // Named rather than left empty, so a run leaves its binary results inside the directory it
    // already owns instead of somewhere EPANET picked.
    let output = path(&report.with_extension("out"))?;
    let network = path(network)?;
    let report = path(report)?;
    let empty = CString::new("").unwrap();
    let mut records = Vec::new();
    let _running = engine()?;

    unsafe {
        let mut project: toolkit::Project = std::ptr::null_mut();
        check(toolkit::EN_createproject(&mut project))?;
        let outcome = (|| -> Result<(), String> {
            check(toolkit::EN_open(project, network.as_ptr(), report.as_ptr(), output.as_ptr()))?;
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
            // Every quality pass rides on the hydraulics EN_initH(SAVE) just recorded, and
            // EN_nextQ advances by hydraulic events, which keeps each on the cadence pressure
            // reports at. The chemical goes first and untouched: the file's own [OPTIONS],
            // [QUALITY], [SOURCES] and [REACTIONS] came back through XTRA, so the project is
            // already set to simulate the user's chemistry and EN_setqualtype would only
            // disturb it.
            if let Some(channel) = quality.chemical {
                quality_pass(project, node_count, nodes, channel, None, (t0_seconds, t1_seconds), &mut records)?;
            }
            // One pass per traced source. Percent is bounded by meaning, not by the toolkit's
            // numerics, and the declaration promises 0..100, so the bounds are enforced here.
            for (source, channel) in &quality.traces {
                let source = CString::new(source.as_bytes()).map_err(|_| "source name is not usable".to_string())?;
                check(toolkit::EN_setqualtype(
                    project,
                    TRACE_ANALYSIS,
                    empty.as_ptr(),
                    empty.as_ptr(),
                    source.as_ptr(),
                ))?;
                quality_pass(
                    project,
                    node_count,
                    nodes,
                    *channel,
                    Some((0.0, 100.0)),
                    (t0_seconds, t1_seconds),
                    &mut records,
                )?;
            }
            // Age last, because EN_setqualtype leaves the project on whatever ran before it.
            // EN_AGE is switched on here so the user's .inp needs no [QUALITY] section.
            let Some(age) = quality.age else { return Ok(()) };
            check(toolkit::EN_setqualtype(project, AGE_ANALYSIS, empty.as_ptr(), empty.as_ptr(), empty.as_ptr()))?;
            quality_pass(project, node_count, nodes, age, None, (t0_seconds, t1_seconds), &mut records)
        })();
        toolkit::EN_close(project);
        toolkit::EN_deleteproject(project);
        outcome?;
    }
    Ok(records)
}

/// One pass of the quality loop over the saved hydraulics, writing every node's value into
/// `channel` for the `window` of seconds the caller asked about. `bounds` clamps where the
/// channel declares a range the toolkit's numerics may overshoot by a rounding error.
unsafe fn quality_pass(
    project: toolkit::Project,
    node_count: c_int,
    nodes: &BTreeMap<String, u64>,
    channel: u16,
    bounds: Option<(f32, f32)>,
    window: (f32, f32),
    records: &mut Vec<u8>,
) -> Result<(), String> {
    unsafe {
        check(toolkit::EN_openQ(project))?;
        check(toolkit::EN_initQ(project, SAVE))?;
        loop {
            let mut now: c_long = 0;
            check(toolkit::EN_runQ(project, &mut now))?;
            let seconds = now as f32;
            if seconds >= window.0 && seconds <= window.1 {
                for index in 1..=node_count {
                    if let Some(identity) = nodes.get(&identifier(project, index, true)?) {
                        let mut measured = value(project, index, QUALITY, true)?;
                        if let Some((low, high)) = bounds {
                            measured = measured.clamp(low, high);
                        }
                        records.extend_from_slice(&pack(*identity, channel, seconds, measured));
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
    }
}

/// One node's pressure at the first reported step at or after `t0`.
///
/// A fire flow is a steady-state question asked at one moment, so this runs the hydraulics to
/// that moment and stops, rather than collecting the day the way `simulate` does.
fn steady_pressure(network: &Path, report: &Path, node: &str, t0_seconds: f32) -> Result<f32, String> {
    let path =
        |value: &Path| CString::new(value.to_string_lossy().as_bytes()).map_err(|_| "path is not usable".to_string());
    let output = path(&report.with_extension("out"))?;
    let network = path(network)?;
    let report = path(report)?;
    let _running = engine()?;

    unsafe {
        let mut project: toolkit::Project = std::ptr::null_mut();
        check(toolkit::EN_createproject(&mut project))?;
        let outcome = (|| -> Result<f32, String> {
            check(toolkit::EN_open(project, network.as_ptr(), report.as_ptr(), output.as_ptr()))?;
            check(toolkit::EN_openH(project))?;
            check(toolkit::EN_initH(project, 0))?;
            let mut count: c_int = 0;
            check(toolkit::EN_getcount(project, NODE_COUNT, &mut count))?;
            let index = (1..=count)
                .find(|index| identifier(project, *index, true).map(|found| found == node).unwrap_or(false))
                .ok_or_else(|| format!("this network has no node named '{node}'"))?;
            let mut found = f32::MIN;
            loop {
                let mut now: c_long = 0;
                check(toolkit::EN_runH(project, &mut now))?;
                if now as f32 >= t0_seconds {
                    found = value(project, index, PRESSURE, true)?;
                    break;
                }
                let mut step: c_long = 0;
                check(toolkit::EN_nextH(project, &mut step))?;
                if step == 0 {
                    break;
                }
            }
            check(toolkit::EN_closeH(project))?;
            Ok(found)
        })();
        toolkit::EN_close(project);
        toolkit::EN_deleteproject(project);
        outcome
    }
}

/// One run of the toolkit at a time.
///
/// `EN_createproject` looks like it makes the engine reentrant and does not: enough of EPANET's
/// state is still global that two projects opened at once refuse each other's networks with
/// error 200. It stayed hidden while every caller ran one simulation; a fire-flow search runs
/// dozens, and the tests turned red immediately. `trama-swmm` serialises its engine for exactly
/// the same reason, so this is the shape the other EPA engine already took.
///
/// ponytail: this makes concurrent solves queue rather than run. Two processes would parallelise
/// where two threads cannot, if a server ever needs the throughput.
fn engine() -> Result<std::sync::MutexGuard<'static, ()>, String> {
    static ENGINE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    ENGINE.lock().map_err(|_| "a previous EPANET run panicked".to_string())
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
