# TRAMA

[![License: BSL 1.1](https://img.shields.io/badge/License-BSL--1.1-orange.svg)](LICENSE)

**Source-available network-map engine for people who do not use GIS.**

TRAMA packages a network graph, pre-tessellated geometry, and typed properties into one portable binary file. It renders network state on the GPU across time and runs analysis through open solver plugins—locally or on a server. The core stays domain-agnostic: it knows nodes, edges, properties, and state channels, not pipes, roads, or power lines.

## The three pillars

- **Open binary format** — range-request friendly, offline-capable, and exportable to GeoJSON, GeoPackage, and MVT.
- **GPU rendering with time** — WebGPU with WebGL2 fallback, GPU state textures, temporal scrub, and network fly-through.
- **Open solver contract** — sandboxed WASM/WASI or server solvers with the same state-delta protocol; EPANET is first.

## Status

**Pre-alpha. Specification in progress.**

Join the early-access list at the project website (deployment pending).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). External pull requests require acceptance of the [Individual Contributor License Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

## License

[Business Source License 1.1](LICENSE). Production use is permitted for internal business operations; offering TRAMA as a hosted or managed service to third parties requires a commercial license. Version 0.0.0-pre-alpha changes to Apache-2.0 on 2030-08-09.
