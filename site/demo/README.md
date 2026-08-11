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
| Solve with EPANET | yes, over WASI |

EPANET is C with a file-based API, so it reaches the browser through WASI rather than
wasm-bindgen — a separate module, given a virtual filesystem holding its input and output.
`WASI_SDK` must point at an unpacked [wasi-sdk](https://github.com/WebAssembly/wasi-sdk/releases);
without it `build.sh` ships the playground without the hydraulic solver rather than failing.

Three things WASI does not have, each met where it appeared: `mkstemp`, which EPANET wants and
the shim supplies in six lines; `std::process::id`, which traps; and `std::env::temp_dir`,
which panics rather than returning a path, so the solver names `/tmp` directly on that target.

## The glue

`proj4rs` declares wasm-bindgen imports on this target — it delegates float parsing to
JavaScript — so the module ships with wasm-bindgen's generated glue rather than the raw C ABI
this started with. That was worth discovering: a hand-rolled ABI would have meant
reimplementing that runtime by hand, and stubbing the imports failed loudly on the first real
`.inp` rather than quietly.
