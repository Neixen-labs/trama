# TRAMA

[![License: BSL 1.1](https://img.shields.io/badge/License-BSL--1.1-orange.svg)](LICENSE)

**Source-available network-map engine for people who do not use GIS.**

TRAMA packages a network graph, pre-tessellated geometry, and typed properties into one portable binary file. It renders network state on the GPU across time and runs analysis through open solver plugins—locally or on a server. The core stays domain-agnostic: it knows nodes, edges, properties, and state channels, not pipes, roads, or power lines.

## The three pillars

- **Open binary format** — range-request friendly, offline-capable, and exportable to GeoJSON, GeoPackage, and MVT.
- **GPU rendering with time** — WebGPU with WebGL2 fallback, GPU state textures, temporal scrub, and network fly-through.
- **Open solver contract** — sandboxed WASM/WASI or server solvers with the same state-delta protocol; EPANET is first.

## Status

**Pre-alpha, and runnable end to end.** A GeoJSON network compiles to a `.trama` file, a browser range-loads it over HTTP, a solver writes state into it, and the demo scrubs time over the result. Every part of that sentence is narrower than it sounds; the table says how.

| Piece | What works today | Not yet |
|---|---|---|
| `compiler/` — `trama-engine` (Python) | GeoJSON `LineString` in; `compile`, `validate`, and `export --to geojson` back out. Byte-identical output for identical input. 49,612 edges compile in 6.0 s to 13% of the equivalent GeoJSON export. | EPANET `.inp`, CSV points, polygons, GeoPackage export |
| `engine/` — `@trama/core` (TypeScript) | Range reader for header, directory, and sections, each checked against its CRC-32C; instanced WebGL2 line renderer with screen-constant width; MapLibre custom layer; state ring buffer feeding an R32F texture; SSE client for solver deltas | WebGPU, OPFS offline cache, fly-through camera, the 100k-segment CI benchmark |
| `solvers/example-diffusion` | Reference solver over HTTP + Server-Sent Events, with a contract test suite that runs against a real server | `solvers/epanet/` is an empty placeholder; no WASM/WASI runtime yet |
| `site/` | Landing page at [trama.build](https://trama.build) | The in-browser playground is phase 5 |

- [File format specification](docs/SPEC.md)
- [Solver contract](docs/SOLVER_CONTRACT.md)
- [Design decisions](docs/DECISIONS.md)
- [Public RFC discussion](https://github.com/Neixen-labs/trama/issues/1)

### Run it

```bash
cd compiler && uv run trama compile ../fixtures/network.geojson out.trama
uv run trama validate out.trama
```

The demo renders a compiled file on a MapLibre basemap and scrubs solver state over it. It needs two terminals, and no bundler:

```bash
cd engine && npm run demo                                    # http://localhost:8790/
cd solvers/example-diffusion && uv run python -m example_diffusion.server
```

Join the early-access list at [trama.build](https://trama.build).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). External pull requests require acceptance of the [Individual Contributor License Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

## License

[Business Source License 1.1](LICENSE). Production use is permitted for internal business operations; offering TRAMA as a hosted or managed service to third parties requires a commercial license. Version 0.0.0-pre-alpha changes to Apache-2.0 on 2030-08-09.
