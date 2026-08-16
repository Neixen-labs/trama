// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The power flow behind the solver contract.
//!
//! The same arithmetic the Python solver runs through pandapower, with no Python: this one is a
//! Rust crate, so it compiles to WASI and runs in the browser. That is the whole point of it.
//! `solvers/pandapower` stays where it is — it is the second implementation that proves the format
//! is legible from outside, and it remains the oracle the tests here are measured against.

use serde_json::Value;
use trama_solver::server::{Rejection, Request, Solver};
use trama_solver::{DELTA_BYTES, declared, pack};

use crate::flow::{self, Failure};
use crate::network;

pub const VOLTAGE_CHANNEL: &str = "voltage";
pub const LOADING_CHANNEL: &str = "loading";
const KNOWN: [&str; 3] = ["load_scaling", "voltage_channel", "loading_channel"];
/// SPEC 4.2 of the contract: node channels are kind 1, edge channels kind 2.
const NODE: u8 = 1;
const EDGE: u8 = 2;

pub struct PowerSolver;

impl Solver for PowerSolver {
    fn id(&self) -> &'static str {
        "power-flow"
    }

    fn contract_versions(&self) -> &'static [&'static str] {
        &["0.1.0", "0.2.0"]
    }

    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection> {
        if let Some(unknown) =
            request.params.as_object().and_then(|params| params.keys().find(|key| !KNOWN.contains(&key.as_str())))
        {
            return Err(Rejection::request(format!("unknown parameter: {unknown}")));
        }
        if request.t1_seconds < request.t0_seconds {
            return Err(Rejection::request("t1_seconds must not precede t0_seconds"));
        }
        let scaling = scaling(&request.params)?;
        let voltage_name = request.params["voltage_channel"].as_str().unwrap_or(VOLTAGE_CHANNEL);
        let loading_name = request.params["loading_channel"].as_str().unwrap_or(LOADING_CHANNEL);
        let voltage = declared(&request.container, voltage_name, NODE).ok();
        let loading = declared(&request.container, loading_name, EDGE).ok();
        if voltage.is_none() && loading.is_none() {
            return Err(Rejection::input(format!(
                "this container declares neither a node channel '{voltage_name}' nor an edge channel '{loading_name}'"
            )));
        }

        let mut deltas = Vec::with_capacity(scaling.len() * 200 * DELTA_BYTES);
        for (step, factor) in scaling.iter().enumerate() {
            // One real load flow per multiplier, spread across the interval. Never an interpolation
            // between two: a curve drawn between two solved instants is not a solved network, and
            // the points where it would differ most are the ones worth running the study for.
            let moment = match scaling.len() {
                1 => request.t0_seconds,
                count => {
                    request.t0_seconds + (request.t1_seconds - request.t0_seconds) * step as f32 / (count - 1) as f32
                }
            };
            let model = network::model(&request.container, *factor).map_err(Rejection::input)?;
            let solution = flow::solve(&model.buses, &model.branches).map_err(|failure| unsolvable(failure, moment))?;

            if let Some(channel) = voltage {
                for (position, entity) in model.bus_entity.iter().enumerate() {
                    // An auxiliary bus is an artefact of an open switch, not something the file has
                    // an id for. It carries a voltage and nobody asked for it.
                    if let Some(id) = entity {
                        deltas.extend_from_slice(&pack(*id, channel, moment, solution.vm_pu[position] as f32));
                    }
                }
            }
            if let Some(channel) = loading {
                for (position, percent) in network::loadings(&model, &solution).iter().enumerate() {
                    // `None` is a branch the source gave no rating: see `network::Rating`.
                    if let Some(percent) = percent {
                        deltas.extend_from_slice(&pack(
                            model.branch_entity[position],
                            channel,
                            moment,
                            *percent as f32,
                        ));
                    }
                }
            }
        }
        Ok(deltas)
    }
}

/// The demand curve is the caller's to supply.
///
/// A load flow is one instant. Turning it into a day means saying what the day looks like, and a
/// solver that invented a profile of its own would be reporting a modelling assumption as a
/// measurement — so the default is one multiplier of 1.0, which is the network as the file has it.
fn scaling(params: &Value) -> Result<Vec<f64>, Rejection> {
    match &params["load_scaling"] {
        Value::Null => Ok(vec![1.0]),
        Value::Array(values) if values.is_empty() => Err(Rejection::request("load_scaling is empty")),
        Value::Array(values) => values
            .iter()
            .map(|value| match value.as_f64() {
                Some(factor) if factor >= 0.0 && factor.is_finite() => Ok(factor),
                _ => Err(Rejection::request("load_scaling holds finite, non-negative numbers")),
            })
            .collect(),
        _ => Err(Rejection::request("load_scaling is an array of numbers")),
    }
}

/// A network that will not solve is 400, not 500: the request was well formed and the network in it
/// has no steady state. The message carries the bus to look at, because that is the actionable part
/// — "it did not converge" leaves an operator with a whole network to search.
fn unsolvable(failure: Failure, moment: f32) -> Rejection {
    Rejection::input(format!("at t={moment}s, {failure}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use trama_format::Importer;

    fn container() -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/networks/oberrhein.json");
        let import = crate::PowerImporter.load(&path, &BTreeMap::new()).unwrap();
        trama_format::compile(&import.features, &import.channels, &import.extras).unwrap()
    }

    /// `Rejection` implements no `Debug`, so refusals are unwrapped here rather than by widening
    /// the contract crate's public shape for the sake of a test.
    fn refusal(result: Result<Vec<u8>, Rejection>) -> Rejection {
        match result {
            Err(rejection) => rejection,
            Ok(deltas) => panic!("expected a refusal, got {} bytes of deltas", deltas.len()),
        }
    }

    fn deltas(result: Result<Vec<u8>, Rejection>) -> Vec<u8> {
        match result {
            Ok(deltas) => deltas,
            Err(rejection) => panic!("{}", rejection.message),
        }
    }

    fn request(params: Value) -> Request {
        Request { container: container(), params, t0_seconds: 0.0, t1_seconds: 86400.0 }
    }

    #[test]
    fn the_manifest_describes_this_solver() {
        let manifest = include_str!("../solver.toml");
        trama_solver::manifest_agrees_with(manifest, PowerSolver.id(), PowerSolver.contract_versions()).unwrap();
    }

    #[test]
    fn one_flow_writes_every_bus_and_every_branch() {
        let deltas = deltas(PowerSolver.solve(&request(serde_json::json!({}))));
        // 179 buses and 183 branches, all rated: no auxiliary bus reaches the file.
        assert_eq!(deltas.len(), (179 + 183) * DELTA_BYTES);
    }

    #[test]
    fn a_curve_runs_one_flow_per_point() {
        let deltas = deltas(PowerSolver.solve(&request(serde_json::json!({"load_scaling": [0.8, 1.0, 1.2]}))));
        assert_eq!(deltas.len(), 3 * (179 + 183) * DELTA_BYTES);

        // The last delta of the first step is at t0 and the first of the last step at t1: the
        // curve spans the interval asked for rather than bunching at one end.
        let at = |index: usize| {
            f32::from_le_bytes(deltas[index * DELTA_BYTES + 10..index * DELTA_BYTES + 14].try_into().unwrap())
        };
        assert_eq!(at(0), 0.0);
        assert_eq!(at(3 * (179 + 183) - 1), 86400.0);
    }

    #[test]
    fn a_parameter_nobody_defined_is_refused_rather_than_ignored() {
        let error = refusal(PowerSolver.solve(&request(serde_json::json!({"load_scalling": [1.0]}))));
        assert_eq!(error.status, 400);
        assert!(error.message.contains("load_scalling"), "{}", error.message);
    }

    #[test]
    fn a_network_that_cannot_hold_a_voltage_says_which_bus() {
        // Scaling the load by a thousand puts the network past the nose of its own PV curve: there
        // is no steady state, and the answer must name where it broke rather than report numbers.
        let error = refusal(PowerSolver.solve(&request(serde_json::json!({"load_scaling": [1000.0]}))));
        assert_eq!(error.status, 400);
        assert!(error.message.contains("bus"), "{}", error.message);
    }
}
