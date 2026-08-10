# trama-engine

Compiler for the TRAMA binary network-map format.

```
trama compile network.geojson network.trama
```

It reads GeoJSON `LineString` features and writes a deterministic `.trama` container with zstd-compressed `GEOM`, `GRPH`, `PROP`, and `STCH` sections. Line endpoints that share a coordinate become one graph node, so the `GRPH` section carries real CSR adjacency. Each edge is stored whole in the deepest tile that contains it; geometry is not clipped at tile borders yet.

Not implemented yet: typed properties, so features with non-empty `properties` are rejected; the render triangle mesh, so `GEOM` carries centerline paths only; other input formats and the exporters.

Keep format behavior aligned with `../docs/SPEC.md` before adding code.
