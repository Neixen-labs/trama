// SPDX-License-Identifier: LicenseRef-BSL-1.1
fn main() {
    let port = std::env::args().nth(1).and_then(|value| value.parse().ok()).unwrap_or(8805);
    if let Err(error) = trama_solver::server::serve(&trama_swmm::solver::SwmmSolver, port) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
