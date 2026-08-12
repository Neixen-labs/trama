// SPDX-License-Identifier: LicenseRef-BSL-1.1
use trama_solver::{manifest_agrees_with, server::Solver};

#[test]
fn the_manifest_describes_the_solver_it_ships_with() {
    let manifest = include_str!("../solver.toml");
    let solver = trama_trace::TraceSolver;

    let outcome = manifest_agrees_with(manifest, solver.id(), solver.contract_versions());

    assert_eq!(outcome, Ok(()));
}
