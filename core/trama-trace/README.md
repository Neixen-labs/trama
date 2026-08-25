# trama-trace

What a network reaches, and what it stops reaching when you cut it: downstream, upstream, reach,
isochrone, isolation, critical edges, source allocation. The first four are one search with three
knobs.

Nothing in it knows what an edge is — it reads like water and is domain-free by design, which is
why the same crate answers "which customers lose supply if this valve shuts" and "how far can a
van get in twenty minutes". It honours turn restrictions from the same column the router reads,
settling arcs rather than nodes, so an isochrone and a route drawn over one container cannot
disagree about which movements exist.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
