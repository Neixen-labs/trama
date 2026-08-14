// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The EPANET solver as a WASI command, so a browser can run it.
//!
//! Files are the interface because they are what EPANET already speaks and what WASI already
//! provides: the host writes a container, runs this, and reads the deltas back.
//!
//!     trama-epanet-wasi <container> <deltas> [t1_seconds] [closed_id,closed_id,...]

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.len() < 3 {
        eprintln!("usage: trama-epanet-wasi <container> <deltas> [t1_seconds]");
        return ExitCode::FAILURE;
    }
    let t1_seconds: f32 = arguments.get(3).and_then(|value| value.parse().ok()).unwrap_or(86400.0);
    let closed: Vec<u64> =
        arguments.get(4).map(|list| list.split(',').filter_map(|id| id.parse().ok()).collect()).unwrap_or_default();
    let outcome = std::fs::read(&arguments[1])
        .map_err(|error| error.to_string())
        .and_then(|container| {
            trama_epanet::solver::solve(&container, "pressure", "flow", "age", &closed, 0.0, t1_seconds)
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
