// SPDX-License-Identifier: LicenseRef-BSL-1.1
#![cfg(feature = "solver")]
use trama_solver::{manifest_agrees_with, server::Solver};

#[test]
fn the_manifest_describes_the_solver_it_ships_with() {
    let manifest = include_str!("../solver.toml");
    let solver = trama_swmm::solver::SwmmSolver;

    let outcome = manifest_agrees_with(manifest, solver.id(), solver.contract_versions());

    assert_eq!(outcome, Ok(()));
}
