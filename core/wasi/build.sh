#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Builds the EPA solvers as WASI commands, which is what lets a browser run them: EPANET and
# SWMM are C with file-based APIs, and only WASI gives them a libc and a filesystem.
#
#   WASI_SDK=/path/to/wasi-sdk-33.0 ./core/wasi/build.sh
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
: "${WASI_SDK:?set WASI_SDK to an unpacked wasi-sdk (https://github.com/WebAssembly/wasi-sdk/releases)}"
export CMAKE_TOOLCHAIN_FILE="$here/toolchain.cmake"
export CC="$WASI_SDK/bin/clang"
# bindgen parses EPANET's headers for this target too, and without the sysroot it silently
# emits constants and no functions: every declaration using a libc type gets skipped.
export BINDGEN_EXTRA_CLANG_ARGS="--sysroot=$WASI_SDK/share/wasi-sysroot -DDLLEXPORT="
export AR="$WASI_SDK/bin/llvm-ar"
# epanet-sys asks for -lm using host cfgs, so it asks here too; WASI keeps its libm in the
# sysroot rather than beside the Rust target's own libraries.
export RUSTFLAGS="${RUSTFLAGS:-} -L native=$WASI_SDK/share/wasi-sysroot/lib/wasm32-wasip1"

cd "$here/.."
cargo build --release --target wasm32-wasip1 -p trama-epanet --bin trama-epanet-wasi
cargo build --release --target wasm32-wasip1 -p trama-swmm --bin trama-swmm-wasi
printf 'built %s epanet, %s swmm\n' \
  "$(du -h target/wasm32-wasip1/release/trama-epanet-wasi.wasm | cut -f1)" \
  "$(du -h target/wasm32-wasip1/release/trama-swmm-wasi.wasm | cut -f1)"
