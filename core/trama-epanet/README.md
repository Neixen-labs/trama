# trama-epanet

Water distribution networks for TRAMA: EPANET `.inp` in and out, and a solver that runs the
EPANET 2.3 toolkit over the compiled container.

It streams pressure, flow, water age, the file's own chemical decaying under its `[REACTIONS]`
block, and source tracing — one channel per source, carrying the share of each node's water that
came from it. It also rates fire flow: how much a hydrant at a given node could deliver while the
network still holds 20 psi, which is a search over simulations rather than a reading of one.

The `solver` feature (on by default) links EPANET statically through `epanet-sys`. Turn it off and
what remains is the importer and exporter, which is what reaches a browser: the engine is C with a
file-based API, and `wasm32-unknown-unknown` has neither libc nor a filesystem to give it.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
