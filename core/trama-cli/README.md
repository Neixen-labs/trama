# trama-cli

The TRAMA command line. Installs one binary, `trama`.

```console
$ cargo install trama-cli
$ trama compile network.inp -o network.trama
$ trama validate network.trama
$ trama export network.trama --to geojson|gpkg|mvt|inp
```

`compile` reads GeoJSON, EPANET `.inp`, EPA SWMM `.inp` (`--importer swmm`), an OpenStreetMap
extract, or a pandapower JSON (`--importer power`), and writes one container. `--points` joins a
CSV of coordinates to the nodes they land on. The GeoPackage export carries stable ids, typed
columns, and the difference between a null and a zero; the MVT export writes a tile pyramid whose
protobuf is hand-encoded, with no dependency for it.

Note that the crate is `trama-cli`: the name `trama` on crates.io belongs to an unrelated project.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
