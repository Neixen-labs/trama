# trama-engine

Compiler for the TRAMA binary network-map format.

```
trama compile network.geojson network.trama
trama export network.trama out --to geojson
```

`compile` reads GeoJSON `LineString` features and writes a deterministic `.trama` container with zstd-compressed `GEOM`, `GRPH`, `PROP`, and `STCH` sections. Line endpoints that share a coordinate become one graph node, so `GRPH` carries real CSR adjacency. Each edge is stored whole in the deepest tile that contains it; geometry is not clipped at tile borders yet. Feature properties become typed `PROP` columns, one per key, with the type inferred from the values and mixtures rejected.

`export` writes `out.nodes.geojson` and `out.edges.geojson`, each feature tagged with `_trama_id`. Recompiling the edge document keeps those edge IDs. Coordinates come back from quantized geometry, so they land within one quantization step of the source rather than on it.

Not implemented yet: node properties, since a LineString has none; the render triangle mesh, so `GEOM` carries centerline paths only; enum columns; state-channel declarations, which solvers own; the EPANET and CSV inputs; the GeoPackage and MVT exporters.

Keep format behavior aligned with `../docs/SPEC.md` before adding code.
