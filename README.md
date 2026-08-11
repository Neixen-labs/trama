# TRAMA

[![License: BSL 1.1](https://img.shields.io/badge/License-BSL--1.1-orange.svg)](LICENSE)

**Source-available network-map engine for people who do not use GIS.**

TRAMA packages a network graph, pre-tessellated geometry, and typed properties into one portable binary file. It renders network state on the GPU across time and runs analysis through open solver plugins—locally or on a server. The core stays domain-agnostic: it knows nodes, edges, properties, and state channels, not pipes, roads, or power lines.

## The three pillars

- **Open binary format** — range-request friendly, offline-capable, and exportable to GeoJSON, GeoPackage, and MVT.
- **GPU rendering with time** — WebGPU with WebGL2 fallback, GPU state textures, temporal scrub, and network fly-through.
- **Open solver contract** — sandboxed WASM/WASI or server solvers with the same state-delta protocol; EPANET is first.

## Status

**Pre-alpha, and runnable end to end.** An EPANET network or a GeoJSON one compiles to a `.trama` file, a browser range-loads it over HTTP, a solver writes state into it, and the demo scrubs time over the result. Every part of that sentence is narrower than it sounds; the table says how.

| Piece | What works today | Not yet |
|---|---|---|
| `compiler/` — `trama-engine` (Python) | GeoJSON and, through an installed importer, EPANET `.inp`; `compile`, `validate`, and `export --to geojson` back out. Byte-identical output for identical input. Typed node and edge properties, declared state channels, and opaque records the core carries without reading. | CSV points, polygons, GeoPackage export |
| `engine/` — `@trama/core` (TypeScript) | Range reader for header, directory, and sections, each checked against its CRC-32C; instanced WebGL2 line renderer with screen-constant width; MapLibre custom layer; state ring buffer feeding an R32F texture; SSE client for solver deltas | WebGPU, OPFS offline cache, fly-through camera |
| `solvers/epanet` | `.inp` import and export, and a solver that runs the OWA-EPANET toolkit and streams pressure and flow. Round trip verified by simulation on Net1 and Net3. | Simulating needs a platform with a wheel; no WASM runtime yet |
| `solvers/example-diffusion` | Reference solver over HTTP + Server-Sent Events, with a contract test suite that runs against a real server | — |
| `site/` | Landing page at [trama.build](https://trama.build) | The in-browser playground is phase 5 |

### Measured

Numbers from `compiler/benchmarks/synthetic_grid.py` and `engine/bench/`, not estimates:

| | Result | Criterion |
|---|---|---|
| Compile 49,612 edges | 5.7 s | under 30 s |
| Container size | 19.0% of the source GeoJSON | under 20% |
| Draw 103,040 segments with animated state | 0.6 ms per frame, p95 0.8 ms | under 16.7 ms |

The frame budget has twenty times the headroom it needs on integrated graphics from 2017. Loading state is where a large network first costs anything: filling sixteen ring slots for 99,904 edges is about 470 ms, or 290 ns per delta.

- [File format specification](docs/SPEC.md)
- [Solver contract](docs/SOLVER_CONTRACT.md)
- [Design decisions](docs/DECISIONS.md)
- [Public RFC discussion](https://github.com/Neixen-labs/trama/issues/1)

### Run it

```bash
cd compiler && uv run trama compile ../fixtures/network.geojson out.trama
uv run trama validate out.trama
```

An EPANET network needs the importer that reads it, and a coordinate reference system, because a `.inp` declares none:

```bash
cd solvers/epanet && uv sync
uv run trama compile tests/networks/Net3.inp net3.trama -o source-crs=EPSG:3857
```

The demo renders a compiled file on a MapLibre basemap and scrubs solver state over it. It needs two terminals, and no bundler:

```bash
cd engine && npm run demo                                    # http://localhost:8790/
cd solvers/example-diffusion && uv run python -m example_diffusion.server
```

To drive it with EPANET instead, run that solver and point the page at it:

```bash
cd solvers/epanet && uv run python -m trama_epanet.server
```

`http://localhost:8790/?file=net3.trama&solver=http://127.0.0.1:8802/solve&step=3600&window=86400`

Join the early-access list at [trama.build](https://trama.build).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). External pull requests require acceptance of the [Individual Contributor License Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

## License

[Business Source License 1.1](LICENSE). Production use is permitted for internal business operations; offering TRAMA as a hosted or managed service to third parties requires a commercial license. Version 0.0.0-pre-alpha changes to Apache-2.0 on 2030-08-09.
