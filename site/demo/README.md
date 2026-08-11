# The playground

Drop a network, watch it compile, see it. The file never leaves the browser: the compiler is
the same Rust that the command line runs, built to WebAssembly.

```bash
./build.sh          # gathers the engine, the compiler and the examples into vendor/
python3 -m http.server --directory .. 8080
```

`vendor/` and `examples/` are builds, not source, and git ignores them. `build.sh` needs a
clang carrying the WebAssembly backend — Linux ships one, macOS wants `brew install llvm` —
and `wasm-bindgen`, which `WASM_BINDGEN=` can point at.

## What runs where

| | In the browser |
|---|---|
| Compile GeoJSON | yes |
| Compile EPANET `.inp` | yes, with the coordinate reference system stated |
| Solve with the example solver | yes |
| Solve with EPANET | **no** |

EPANET is C with a file-based API, and `wasm32-unknown-unknown` gives it neither libc nor a
filesystem. Running it in a browser needs WASI and a virtual filesystem, which is a project of
its own; until then a real hydraulic result comes from `trama-solver-epanet` running locally.

The page says so in its own words rather than letting a visitor assume the pulse it draws is
their network's hydraulics.

## The glue

`proj4rs` declares wasm-bindgen imports on this target — it delegates float parsing to
JavaScript — so the module ships with wasm-bindgen's generated glue rather than the raw C ABI
this started with. That was worth discovering: a hand-rolled ABI would have meant
reimplementing that runtime by hand, and stubbing the imports failed loudly on the first real
`.inp` rather than quietly.
