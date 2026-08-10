# trama-engine

Compiler for the TRAMA binary network-map format.

The first vertical slice accepts one GeoJSON `LineString` feature and writes a deterministic `.trama` container with zstd-compressed `GEOM`, `GRPH`, `PROP`, and `STCH` sections. Edge properties support finite numbers, strings, and booleans; multiple features and lines spanning tiles are rejected until their graph and tile encodings are implemented.

Keep format behavior aligned with `../docs/SPEC.md` before adding code.
