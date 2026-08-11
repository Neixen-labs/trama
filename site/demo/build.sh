#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Gathers what the playground serves. Everything it produces is ignored by git: the page is
# source, its vendor directory is a build.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$here/../.."
vendor="$here/vendor"
rm -rf "$vendor" && mkdir -p "$vendor/engine" "$here/examples"

echo "engine"
(cd "$root/engine" && npm ci --silent && npm run build --silent)
cp "$root"/engine/dist/*.js "$vendor/engine/"
cp "$root"/engine/node_modules/maplibre-gl/dist/*.mjs "$vendor/"  # maplibre-gl.mjs pulls a shared chunk beside it
cp "$root/engine/node_modules/maplibre-gl/dist/maplibre-gl.css" "$vendor/maplibre-gl.css"
cp "$root/engine/node_modules/fzstd/esm/index.mjs" "$vendor/fzstd.mjs"
cp -r "$root/engine/node_modules/@bjorn3/browser_wasi_shim/dist" "$vendor/wasi"

echo "compiler"
# zstd is C, so this needs a clang carrying the WebAssembly backend: Linux ships one, macOS
# does not, which is what `brew install llvm` is for.
clang="${CC:-$(command -v /usr/local/opt/llvm/bin/clang || command -v clang)}"
(cd "$root/core" && CC="$clang" AR="$(dirname "$clang")/llvm-ar" \
  cargo build --release --target wasm32-unknown-unknown -p trama-wasm)
# proj4rs declares wasm-bindgen imports on this target, so the module needs its glue: a
# hand-rolled C ABI would have meant reimplementing that runtime.
"${WASM_BINDGEN:-wasm-bindgen}" --target web --no-typescript \
  --out-dir "$vendor" "$root/core/target/wasm32-unknown-unknown/release/trama_wasm.wasm"

# EPANET is C with a file-based API, so it reaches the browser through WASI rather than
# wasm-bindgen: a separate module, with a virtual filesystem for its input and output.
if [ -n "${WASI_SDK:-}" ]; then
  echo "epanet"
  (cd "$root/core" && ./wasi/build.sh)
  cp "$root/core/target/wasm32-wasip1/release/trama-epanet-wasi.wasm" "$vendor/"
else
  echo "epanet: skipped, set WASI_SDK to include the hydraulic solver"
fi

echo "examples"
cp "$root/core/trama-epanet/tests/networks/Net3.inp" "$here/examples/net3.inp"
cp "$root/fixtures/network.geojson" "$here/examples/network.geojson"

printf 'ready: %s of compiler\n' "$(du -h "$vendor/trama_wasm_bg.wasm" | cut -f1)"
