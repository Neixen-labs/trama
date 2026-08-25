# trama-swmm

Drainage networks for TRAMA: an EPA SWMM `.inp` in and out — junctions, outfalls, storage,
conduits, pumps, weirs — with the hydrology carried through unread, and a solver that runs
EPA SWMM 5.2.4 to stream depth, flow, and flooding, the overflow rate at each node.

The command line reaches it as `trama compile --importer swmm`, since EPANET owns the `.inp`
suffix.

The `solver` feature (on by default) links the vendored engine through `trama-swmm-sys`. Without
it there is no C here at all: the dependency on `trama-epanet` is for the bracketed-section text
shape both EPA formats share, and for reprojection.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
