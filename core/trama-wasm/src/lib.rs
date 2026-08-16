// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Compiling in a browser, through the same code the command line runs.
//!
//! The glue is wasm-bindgen's rather than hand-rolled: `proj4rs` already declares its imports
//! on this target, so a raw C ABI would have meant reimplementing that runtime by hand.

use serde_json::Value;
use wasm_bindgen::prelude::*;

/// Compile a GeoJSON FeatureCollection into a container.
#[wasm_bindgen]
pub fn compile_geojson(source: &str) -> Result<Vec<u8>, JsError> {
    let parsed: Value = serde_json::from_str(source).map_err(|error| JsError::new(&error.to_string()))?;
    let features = parsed["features"].as_array().cloned().unwrap_or_default();
    trama_format::compile(&features, &[], &[]).map_err(|error| JsError::new(&error))
}

/// Compile an EPANET `.inp`. `crs` names the coordinate reference system its numbers are in,
/// because a `.inp` declares none and guessing is the failure mode SPEC 4.2 warns about.
#[wasm_bindgen]
pub fn compile_inp(source: &str, crs: &str) -> Result<Vec<u8>, JsError> {
    let imported = trama_epanet::importer::import(source, crs).map_err(|error| JsError::new(&error))?;
    trama_format::compile(&imported.features, &imported.channels, &imported.extras)
        .map_err(|error| JsError::new(&error))
}

/// Compile a SWMM `.inp`, the other EPA dialect sharing the suffix. The page tries EPANET
/// first and retries here when the redirect message names SWMM, the same dance the command
/// line does with `--importer swmm`.
#[wasm_bindgen]
pub fn compile_swmm(source: &str, crs: &str) -> Result<Vec<u8>, JsError> {
    let imported = trama_swmm::importer::import(source, crs).map_err(|error| JsError::new(&error))?;
    trama_format::compile(&imported.features, &imported.channels, &imported.extras)
        .map_err(|error| JsError::new(&error))
}

/// Compile an OpenStreetMap extract, as Overpass writes it for `out geom;`.
///
/// The road importer declares the channel a router writes, so a container built here is
/// solvable without the caller knowing what `on_route` is.
#[wasm_bindgen]
pub fn compile_osm(source: &str) -> Result<Vec<u8>, JsError> {
    let imported = trama_roads::import(source).map_err(|error| JsError::new(&error))?;
    trama_format::compile(&imported.features, &imported.channels, &imported.extras)
        .map_err(|error| JsError::new(&error))
}

/// Compile a pandapower network, as `pandapower.to_json` writes one.
///
/// It needs no CRS: pandapower stores each row's geometry as GeoJSON, which is WGS 84 by
/// definition, so unlike an EPANET `.inp` the file already says where it is.
#[wasm_bindgen]
pub fn compile_power(source: &str) -> Result<Vec<u8>, JsError> {
    let imported = trama_power::import(source).map_err(|error| JsError::new(&error))?;
    trama_format::compile(&imported.features, &imported.channels, &imported.extras)
        .map_err(|error| JsError::new(&error))
}

/// Route through `waypoints`, given as node indices, and return the packed deltas.
///
/// Unlike EPANET, this solver is Rust with no filesystem in its API, so it needs no WASI: the
/// same code the server runs compiles straight into this module.
///
/// `speed_property` names a `PROP` column holding each edge's own speed in metres per second.
/// With one the search minimises time, without one distance, and `speed` is the fallback for an
/// edge whose column holds nothing usable.
#[wasm_bindgen]
pub fn solve_route(
    container: &[u8],
    waypoints: &[u32],
    speed: f32,
    speed_property: Option<String>,
    t1_seconds: f32,
) -> Result<Vec<u8>, JsError> {
    let parameters = trama_routing::Parameters {
        waypoints: waypoints.iter().map(|index| *index as usize).collect(),
        speed_metres_per_second: speed,
        speed_property,
        ..Default::default()
    };
    trama_routing::solve(container, &parameters, 0.0, t1_seconds).map_err(|error| JsError::new(&error))
}

/// How far a vehicle gets from `seeds` within the window, as a spreading progression.
///
/// An isochrone is the same search as a route, stopped by a budget instead of a destination, and
/// the budget here is the scrub's own window: the spread and the clock are the same number.
#[wasm_bindgen]
pub fn solve_reach(
    container: &[u8],
    seeds: &[u32],
    speed: f32,
    speed_property: Option<String>,
    t1_seconds: f32,
) -> Result<Vec<u8>, JsError> {
    let parameters = trama_trace::Parameters {
        channel: "reach".into(),
        operation: trama_trace::Operation::Trace {
            seeds: seeds.iter().map(|index| *index as usize).collect(),
            direction: trama_trace::Direction::Forward,
            budget: Some(t1_seconds as f64),
        },
        cost: trama_trace::Cost::Seconds { metres_per_second: speed as f64, speed_property },
        step_seconds: 60.0,
    };
    trama_trace::solve(container, &parameters, 0.0, t1_seconds).map_err(|error| JsError::new(&error))
}

/// What loses service when `cut` is removed, seen from `seeds`.
///
/// The question a utility actually asks — close this valve, who runs dry — and a street network
/// answers it in the same call: close this road, what is cut off.
#[wasm_bindgen]
pub fn solve_isolation(container: &[u8], seeds: &[u32], cut: &[u32], t1_seconds: f32) -> Result<Vec<u8>, JsError> {
    let parameters = trama_trace::Parameters {
        channel: "isolated".into(),
        operation: trama_trace::Operation::Isolation {
            cut: cut.iter().map(|index| *index as usize).collect(),
            seeds: seeds.iter().map(|index| *index as usize).collect(),
            // Service reaches wherever the network connects, whichever way a street happens to
            // run: a one-way street still has water under it.
            direction: trama_trace::Direction::Both,
        },
        cost: trama_trace::Cost::Hops,
        step_seconds: 60.0,
    };
    trama_trace::solve(container, &parameters, 0.0, t1_seconds).map_err(|error| JsError::new(&error))
}

/// The streets that are the only way through: cutting one splits the network.
///
/// No seeds and no cost — it is a property of the shape alone, which is why it needs nothing
/// clicked on the map.
#[wasm_bindgen]
pub fn solve_critical(container: &[u8], t1_seconds: f32) -> Result<Vec<u8>, JsError> {
    let parameters = trama_trace::Parameters {
        channel: "critical".into(),
        operation: trama_trace::Operation::Critical,
        cost: trama_trace::Cost::Hops,
        step_seconds: 60.0,
    };
    trama_trace::solve(container, &parameters, 0.0, t1_seconds).map_err(|error| JsError::new(&error))
}

/// Run the example solver over a container and return its packed deltas.
#[wasm_bindgen]
pub fn solve(container: &[u8], t1_seconds: f32) -> Result<Vec<u8>, JsError> {
    trama_example::solve(container, &trama_example::Parameters::default(), 0.0, t1_seconds)
        .map_err(|error| JsError::new(&error))
}

/// Solve the load flow of a compiled pandapower network, in the browser.
///
/// The one calculation the playground could not run without a server, because the only solver for
/// it was Python. This is the same Rust the `trama-solver-power` binary runs, so the answer does
/// not depend on which side of the network boundary it was computed — and now there is no boundary
/// to cross: a file dropped on the page is solved on the machine it was dropped from.
///
/// `load_scaling` is the demand curve, one real load flow per multiplier spread across the window.
/// Empty means a single flow at t0, the network as the file has it.
#[wasm_bindgen]
pub fn solve_power(container: &[u8], load_scaling: &[f32], t1_seconds: f32) -> Result<Vec<u8>, JsError> {
    let params = if load_scaling.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::json!({ "load_scaling": load_scaling.iter().map(|factor| *factor as f64).collect::<Vec<f64>>() })
    };
    let request = trama_solver::server::Request { container: container.to_vec(), params, t0_seconds: 0.0, t1_seconds };
    trama_solver::server::Solver::solve(&trama_power::solver::PowerSolver, &request)
        .map_err(|rejection| JsError::new(&rejection.message))
}

/// The largest short-circuit current at every bus of a compiled pandapower network, in kA.
///
/// The second question a distribution utility answers in writing, after the load flow: what a
/// breaker at each point must be able to interrupt. IEC 60909's maximum case, which takes no
/// demand curve because it deliberately ignores what the network happened to be doing.
#[wasm_bindgen]
pub fn solve_fault(container: &[u8]) -> Result<Vec<u8>, JsError> {
    let request = trama_solver::server::Request {
        container: container.to_vec(),
        params: serde_json::json!({ "study": "fault" }),
        t0_seconds: 0.0,
        t1_seconds: 0.0,
    };
    trama_solver::server::Solver::solve(&trama_power::solver::PowerSolver, &request)
        .map_err(|rejection| JsError::new(&rejection.message))
}
