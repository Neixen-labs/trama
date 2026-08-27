// SPDX-License-Identifier: BUSL-1.1
//! The EPANET solver as a WASI command, so a browser can run it.
//!
//! Files are the interface because they are what EPANET already speaks and what WASI already
//! provides: the host writes a container, runs this, and reads the deltas back.
//!
//!     trama-epanet-wasi <container> <deltas> [t1_seconds] [closed_id,...] [fire_id,...]
//!
//! The last two are lists of stable entity ids: edges to shut, and nodes to rate for fire flow.
//! Both are optional and independent, and the fire-flow study is run under the closures — "what
//! can this hydrant deliver with that valve shut" is what a real study asks.

use std::process::ExitCode;

/// A comma-separated list of stable entity ids, as the browser spells them on the command line.
fn ids(argument: Option<&String>) -> Vec<u64> {
    argument.map(|list| list.split(',').filter_map(|id| id.trim().parse().ok()).collect()).unwrap_or_default()
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.len() < 3 {
        eprintln!("usage: trama-epanet-wasi <container> <deltas> [t1_seconds] [closed_id,...] [fire_id,...]");
        return ExitCode::FAILURE;
    }
    let t1_seconds: f32 = arguments.get(3).and_then(|value| value.parse().ok()).unwrap_or(86400.0);
    let closed = ids(arguments.get(4));
    let fire_nodes = ids(arguments.get(5));
    let outcome = std::fs::read(&arguments[1])
        .map_err(|error| error.to_string())
        .and_then(|container| {
            let mut deltas =
                trama_epanet::solver::solve(&container, "pressure", "flow", "age", &closed, 0.0, t1_seconds)?;
            // The same order the HTTP solver uses: the hydraulics first, then the rating that
            // rides on the same run. A file declaring no `fire_flow` channel, or a call naming no
            // hydrants, adds nothing — so the common case pays for neither.
            deltas.extend(trama_epanet::solver::fire_flow(&container, "fire_flow", &fire_nodes, None, &closed, 0.0)?);
            Ok(deltas)
        })
        .and_then(|deltas| std::fs::write(&arguments[2], deltas).map_err(|error| error.to_string()));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
