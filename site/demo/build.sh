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
# `ar` must be LLVM's: GNU ar cannot archive WebAssembly objects. Debian ships it beside clang
# with a version suffix and no bare alias, which is why guessing `<clang dir>/llvm-ar` fails on a
# CI runner while working on a Homebrew install.
here_ar="$(dirname "$clang")/llvm-ar"
archiver="${AR:-}"
if [ -z "$archiver" ]; then
  for candidate in "$here_ar" "$(command -v llvm-ar || true)" $(ls -1 "$(dirname "$clang")"/llvm-ar-* /usr/bin/llvm-ar-* 2>/dev/null | sort -V -r); do
    if [ -x "$candidate" ]; then archiver="$candidate"; break; fi
  done
fi
if [ -z "$archiver" ]; then
  echo "no llvm-ar found beside $clang or on PATH; install LLVM or set AR" >&2
  exit 1
fi
(cd "$root/core" && CC="$clang" AR="$archiver" \
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
# OpenStreetMap data, © OpenStreetMap contributors, ODbL. See fixtures/README.md.
cp "$root/fixtures/madrid.osm.json" "$here/examples/madrid.osm.json"

echo "service worker"
# The precache list is generated because it cannot be written by hand: whether the EPANET module
# is in it depends on whether this build had a WASI SDK. The version is the bytes themselves, so
# a deploy that changes nothing keeps the same cache and one that changes anything replaces it.
assets="$(cd "$here" && find vendor examples -type f ! -name "*.tsbuildinfo" ! -name "*.map" | LC_ALL=C sort | sed 's|^|"./|; s|$|",|' | tr -d '\n')"
version="$(cd "$here" && find vendor examples -type f ! -name "*.tsbuildinfo" ! -name "*.map" | LC_ALL=C sort | xargs shasum | shasum | cut -c1-12)"
sed -e "s|__ASSETS__|[\"./\", ${assets%,}]|" -e "s|__VERSION__|$version|" \
  "$here/sw.template.js" > "$here/sw.js"

printf 'ready: %s of compiler, %s cached offline\n' \
  "$(du -h "$vendor/trama_wasm_bg.wasm" | cut -f1)" \
  "$(cd "$here" && du -ch vendor examples | tail -1 | cut -f1)"
