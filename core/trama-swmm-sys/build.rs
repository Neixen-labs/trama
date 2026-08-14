// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Compiles the vendored EPA SWMM 5.2.4 solver and links it statically.
//!
//! Plain `cc` rather than SWMM's CMake: the engine is C99 with no configuration step, and a
//! second build system would only add a tool for CI to be missing. OpenMP is not enabled — the
//! code guards every use behind `_OPENMP`, and a deterministic single thread is the right
//! default for a solver whose output feeds byte-compared tests.

fn main() {
    let sources = std::fs::read_dir("SWMM")
        .expect("vendored SWMM source at trama-swmm-sys/SWMM")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|extension| extension == "c"));

    let mut build = cc::Build::new();
    build.files(sources).include("SWMM").include("SWMM/include").warnings(false);
    // The engine reads text with the C locale's functions; nothing here needs more than that.
    build.flag_if_supported("-fno-fast-math");
    build.compile("swmm5");

    println!("cargo:rerun-if-changed=SWMM");
}
