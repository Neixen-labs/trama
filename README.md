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
| `core/trama-format` (Rust) | The container: writer, reader, GeoJSON export in WGS 84 or in projected metres. Byte-identical output for identical input, typed node and edge properties, declared state channels, and opaque records it carries without reading. | polygons, deferred to v1 |
| `core/trama-cli` (Rust) | `trama compile`, `validate`, `export --to geojson|gpkg|inp`, and the grid generator behind the benchmarks. The GeoPackage carries stable IDs, typed columns and the difference between a null and a zero, and GDAL opens it: 3,649 LineStrings, 2,770 Points, `EPSG:3857` read back from the file's own definition. `--points` joins a CSV of coordinates to the nodes they land on. | — |
| `core/trama-epanet` (Rust) | `.inp` import and export, and a solver that runs the EPANET 2.3 toolkit and streams pressure and flow, natively or in the browser through WASI. Round trip verified by simulation on Net1 and Net3. | — |
| `core/trama-trace` (Rust) | What a network reaches and what a cut takes out: downstream, upstream, reach, isochrone, isolation, critical edges, source allocation. One search with three knobs for the first four. Nothing in it knows what an edge is. | polygon cuts |
| `core/trama-roads` (Rust) | Reads an OpenStreetMap extract: translates every spelling of `oneway`, normalises `maxspeed` into a speed column, splits ways at the junctions they cross, and declares the channel a router writes. | turn restrictions |
| `core/trama-routing` (Rust) | Fastest paths over the graph, costed by a speed column or by distance, honouring one-way edges, written into a state channel as a vehicle's progress. The second domain, and the evidence the core is not shaped around water. | VRP |
| `core/trama-example` (Rust) | Reference solver over HTTP + Server-Sent Events, to keep the contract under a real implementation. | — |
| `core/trama-wasm` (Rust) | The compiler in a browser, byte-identical to the command line: GeoJSON, EPANET `.inp` and OpenStreetMap extracts, plus the routing solver. 319 kB brotli. | — |
| `engine/` — `@trama/core` (TypeScript) | Range reader for header, directory, and sections, each checked against its CRC-32C; instanced WebGL2 line renderer with screen-constant width; MapLibre custom layer; state ring buffer feeding an R32F texture; SSE client for solver deltas; fly-through camera that tours the graph; OPFS cache so a container read once needs no network again | WebGPU, deferred until MapLibre can hand a custom layer that context |
| `site/` | Landing page at [trama.build](https://trama.build), and a playground that compiles a network in the browser and solves it: EPANET over WASI, or a fastest path between points picked on the map. A service worker precaches it, so it compiles and routes with the server switched off, and the compiled container and its GeoJSON export download straight from the page. | — |

### Measured

Numbers from `cargo run --release -p trama-cli --bin grid -- --report` and `engine/bench/`, not estimates:

| | Result | Criterion |
|---|---|---|
| Compile 49,612 edges | 3.0 s | under 30 s |
| Container size | 12.1% of the equivalent export, 20.9% of a compact hand-written source | under 20% |
| Draw 103,040 segments with animated state | 0.6 ms per frame, p95 0.8 ms | under 16.7 ms |
| Land on the site, drop an EPANET `.inp`, watch it simulated | 1.6 s, or 6.3 s on a throttled phone | under 60 s |

The frame budget has twenty times the headroom it needs on integrated graphics from 2017. Loading state is where a large network first costs anything: filling sixteen ring slots for 99,904 edges is about 470 ms, or 290 ns per delta.

The last row is the one the launch depends on, so `engine/bench/journey.mjs` walks it against the deployed site rather than a local build: arrive cold with no cache and no service worker, drop `Net3.inp`, let EPANET run over WASI, and scrub. `--slow` throttles to four times the CPU cost and a 1.6 Mbps link with 150 ms of latency, which is where the 6.3 s comes from. At that speed the network's size stops mattering — a 3,649-edge city takes 6.6 s, three tenths more than a 119-pipe one — because what a visitor waits for is 707 kB arriving, 399 of them the compiler itself.

The size criterion has two honest answers because "the equivalent GeoJSON" is ambiguous. Against the export — same entities, same IDs, full precision — the container is 12.1%. Against a compact hand-written source, which omits nodes and rounds coordinates, it is 20.9% and misses the target.

- [File format specification](docs/SPEC.md)
- [Solver contract](docs/SOLVER_CONTRACT.md)
- [Design decisions](docs/DECISIONS.md)
- [Public RFC discussion](https://github.com/Neixen-labs/trama/issues/1)

### Install

Nothing is published yet — the release workflow is written and waits on a tag and an `NPM_TOKEN`. When it runs:

```bash
npm install @trama/core            # the browser runtime, no runtime dependencies
```

and `trama`, the compiler and command line, arrives as a binary for Linux, macOS (Intel and Apple silicon) and Windows on the [releases page](https://github.com/Neixen-labs/trama/releases). Until then, build it:

### Run it

```bash
cd core && cargo build --release
./target/release/trama compile ../fixtures/network.geojson out.trama
./target/release/trama validate out.trama
```

What another system knows about a place arrives as a CSV, joined to the network by location. Each row's columns become properties of the node its coordinates land on, typed as the values allow and keeping an empty cell distinct from a zero:

```bash
./target/release/trama compile red.geojson red.trama --points contadores.csv
```

A row whose coordinates match no node stops the compile and says where it was. That is deliberate: the join is on the quantization cell the file stores, about 4 cm, so a meter measured three metres from the junction is a mismatch — and a compiler that quietly dropped it would report success while losing the row.

Your data leaves the same way it came in. GeoJSON exports as two FeatureCollections in WGS 84; GeoPackage exports as one SQLite database of `nodes` and `edges` layers in `EPSG:3857`, which is what QGIS and anything built on GDAL read:

```bash
./target/release/trama export out.trama out-geojson/ --to geojson
./target/release/trama export ../fixtures/teruel.trama teruel.gpkg --to gpkg
```

A whole city is 234 kB as a container against 1.1 MB as the GeoPackage it exports to — the anti-lock-in promise costs something to leave with, which is the point of measuring it.

An EPANET network needs a coordinate reference system, because a `.inp` declares none:

```bash
./target/release/trama compile ../core/trama-epanet/tests/networks/Net3.inp net3.trama -o source-crs=EPSG:3857
```

The demo renders a compiled file on a MapLibre basemap and scrubs solver state over it. It needs two terminals, and no bundler:

```bash
cd engine && npm run demo                    # http://localhost:8790/
cd core && ./target/release/trama-solver-example
```

Routing is the second domain. A road network comes from an OpenStreetMap extract, which Overpass writes as `.json` — a suffix the compiler already claims, so the importer is asked for by name:

```bash
curl -s -X POST https://overpass-api.de/api/interpreter --data-urlencode \
  'data=[out:json][timeout:80];way["highway"~"^(residential|primary|secondary|tertiary)$"](40.4100,-3.7120,40.4230,-3.6950);out geom;' \
  -o city.json
./target/release/trama compile --importer roads city.json city.trama
./target/release/trama-solver-routing          # http://127.0.0.1:8803/solve
./target/release/trama-solver-trace            # http://127.0.0.1:8804/solve
```

The same container answers questions that are not about roads at all. `trama-trace` asks what a
network reaches — downstream, upstream, connected reach and isochrone are one search with three
knobs — and what it stops reaching when it is cut: isolation, critical edges, source allocation.
None of it needs a domain: "upstream" is water's name for searching against the arrows, and a
street network answers "what is cut off if I close this" in the same call a pipe network does.

`out geom;` matters: the importer needs each way's node references to split it where other ways cross it, and without that a crossing never becomes a junction. The importer declares the `on_route` channel itself, so no `--channels` is needed.

`params` takes `waypoints`, the node indices to visit in order. They are positions in the graph's node array, which is sorted by stable ID rather than by input order, so a client reads them from the graph instead of guessing.

`params.speed_property` names a `PROP` column holding each edge's speed in metres per second — `roads:speed_ms` for a network from the road importer. With it the search minimises travel time; without it, distance. On the sample extract 9 of 52 node pairs take different streets depending on which.

To drive the demo with EPANET instead, run that solver and point the page at it:

```bash
cd core && ./target/release/trama-solver-epanet
```

`http://localhost:8790/?file=net3.trama&solver=http://127.0.0.1:8802/solve&step=3600&window=86400`

Join the early-access list at [trama.build](https://trama.build).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). External pull requests require acceptance of the [Individual Contributor License Agreement](CONTRIBUTOR_LICENSE_AGREEMENT.md).

## License

[Business Source License 1.1](LICENSE). Production use is permitted for internal business operations; offering TRAMA as a hosted or managed service to third parties requires a commercial license. Version 0.0.0-pre-alpha changes to Apache-2.0 on 2030-08-09.
