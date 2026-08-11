# trama-solver-epanet

EPANET `.inp` import and export for TRAMA, following `docs/EPANET_BOUNDARY.md`.

This package holds every hydraulic concept in the project. `compiler/` knows nodes, edges,
typed properties, and opaque records; it discovers this package through the `trama.importers`
entry point and never learns what a pump is.

## Use

```bash
uv sync
uv run trama compile network.inp network.trama -o source-crs=EPSG:25830
```

A `.inp` declares no coordinate reference system, so `source-crs` is required. Nothing in the
file says whether its numbers are metres, feet, or a national grid, and a network placed in the
wrong hemisphere still draws perfectly — the failure would be invisible.

Export is `trama_epanet.exporter.export_inp(container, destination, crs)`, given the same CRS.

## What goes where

| `.inp` content | Lands in |
|---|---|
| `[JUNCTIONS]`, `[RESERVOIRS]`, `[TANKS]` | nodes, with `epanet:*` properties |
| `[PIPES]`, `[PUMPS]`, `[VALVES]` | edges, with `epanet:*` properties |
| `[COORDINATES]`, `[VERTICES]` | geometry, reprojected to Web Mercator |
| everything else | one `XTRA` record owned by `epanet`, carried verbatim and never parsed by the core |

A pattern, a pump curve, and a control rule are not values attached to an entity, so SPEC 7.1
will not have them faked as properties. They travel as text and come back as text.

## Solving

```bash
uv run python -m trama_epanet.server        # http://127.0.0.1:8802/solve
```

The solver rebuilds a `.inp` from the container, runs the OWA-EPANET toolkit, and emits one
18-byte delta per entity per reported timestep: node pressure on the `pressure` channel, link
flow on `flow`, each keyed to the entity's stable `u64`. The engine cannot tell it from any
other solver, which is the point of the contract.

That the solver goes through the exporter is deliberate: if the round trip is faithful, the
network being solved is the user's own, and one test proves it by comparing every delta with a
direct toolkit run of the original file.

## Round trip

`.inp → .trama → .inp` is verified by simulation, not by bytes: both files run through the
same EPANET binary and must agree on every node pressure and link flow at every reported
timestep. Byte equality would fail on comments and field spacing while missing a dropped
pattern.

Two things the round trip does not preserve, both deliberate:

- **Coordinate precision.** Geometry is quantized to about 4 cm, so exported coordinates are
  not the source numbers to the last digit. EPANET's hydraulics never read a coordinate.
- **Vertex count.** Geometry is stored per tile, so a link crossing a tile boundary comes back
  with that boundary as an extra vertex, lying exactly on the original segment. A test pins
  this rather than hiding it.

## Test networks

`tests/networks/Net1.inp` and `Net3.inp` are the EPANET example networks distributed with
[OpenWaterAnalytics/EPANET](https://github.com/OpenWaterAnalytics/EPANET), originally from the
US EPA. They are included unmodified.

## Verification dependency

`owa-epanet` runs the comparison. It publishes a `cp312` manylinux wheel, so CI installs it
without a toolchain. There is no macOS wheel for this Python; building locally wants `swig` and
`ninja` on `PATH`.
