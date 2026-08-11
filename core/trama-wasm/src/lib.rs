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

/// Run the example solver over a container and return its packed deltas.
#[wasm_bindgen]
pub fn solve(container: &[u8], t1_seconds: f32) -> Result<Vec<u8>, JsError> {
    trama_example::solve(container, &trama_example::Parameters::default(), 0.0, t1_seconds)
        .map_err(|error| JsError::new(&error))
}
