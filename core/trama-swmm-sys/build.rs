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
    // EPANET and SWMM share EPA ancestry, and both define these utility functions with global
    // linkage. Any binary linking both engines — the CLI does — collides on them, so SWMM's
    // get a prefix. None are part of the swmm5.h API; macOS's linker shrugged, Linux's lld
    // refused, and lld is right.
    for symbol in ["match", "findmatch", "strcomp"] {
        build.define(symbol, format!("swmm_{symbol}").as_str());
    }
    // The engine reads text with the C locale's functions; nothing here needs more than that.
    build.flag_if_supported("-fno-fast-math");
    // WASI's libc has no mkstemp, and SWMM wants one for its scratch files (swmm5.c). The same
    // shim EPANET's WASI build uses, included from the shared core/wasi directory.
    if std::env::var("TARGET").unwrap_or_default().contains("wasi") {
        build.flag("-include").flag("../wasi/mkstemp-shim.h");
    }
    build.compile("swmm5");

    println!("cargo:rerun-if-changed=SWMM");
}
