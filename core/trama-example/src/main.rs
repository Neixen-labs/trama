// SPDX-License-Identifier: LicenseRef-BSL-1.1
fn main() {
    let port = std::env::args().nth(1).and_then(|value| value.parse().ok()).unwrap_or(8801);
    if let Err(error) = trama_solver::server::serve(&trama_example::ExampleSolver, port) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
