# trama-engine

Compiler for the TRAMA binary network-map format.

```
trama compile network.geojson network.trama
trama compile network.inp network.trama
trama export network.trama out --to geojson
trama export network.trama out.inp --to epanet
```

`compile` picks its reader from the source suffix: `.geojson`/`.json` or EPANET `.inp`.

The GeoJSON reader takes `LineString` features and derives one node per distinct snapped endpoint. Either reader produces a deterministic `.trama` container with zstd-compressed `GEOM`, `GRPH`, `PROP`, and `STCH` sections. Line endpoints that share a coordinate become one graph node, so `GRPH` carries real CSR adjacency. Each edge is stored whole in the deepest tile that contains it; geometry is not clipped at tile borders yet. Feature properties become typed `PROP` columns, one per key, with the type inferred from the values and mixtures rejected.

The EPANET reader maps junctions, reservoirs, tanks, pipes, pumps, and valves to nodes and edges, keeping each entity's EPANET ID as a `name` property, its kind as `type`, and node positions as exact `x`/`y` properties. `[COORDINATES]` must be WGS 84 degrees. Simulation sections — patterns, curves, controls, options, times — are not modelled, so they are kept verbatim in the container's optional `SRCE` section and replayed on export around a graph regenerated from the container. An `.inp` round trip therefore returns a simulable model, not just a topology.

`export --to geojson` writes `out.nodes.geojson` and `out.edges.geojson`, each feature tagged with `_trama_id`. Recompiling the edge document keeps those edge IDs. Coordinates come back from quantized geometry, so they land within one quantization step of the source rather than on it.

Not implemented yet: the render triangle mesh, so `GEOM` carries centerline paths only; enum columns; state-channel declarations, which solvers own; the CSV input; the GeoPackage and MVT exporters.

Keep format behavior aligned with `../docs/SPEC.md` before adding code.
