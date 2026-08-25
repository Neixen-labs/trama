# trama-format

The TRAMA v0 container: one binary file with a header and a section directory of offsets, so a
client can fetch only the section it needs over an HTTP range request. Writer, reader, and
GeoJSON export.

The file holds `GEOMETRY` (pre-tessellated tile buffers), `GRAPH` (stable u64 ids and CSR
adjacency), `PROPS` (typed key-values against a global key dictionary), and `STATE_CHANNELS` —
which declares what a solver may write, never the values themselves. Identical input compiles to
identical bytes.

The format is specified in [`docs/SPEC.md`](https://github.com/Neixen-labs/trama/blob/main/docs/SPEC.md),
and the specification leads this crate rather than describing it: `solvers/pandapower` is a
second, independent reader written from the spec alone.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE) — internal
production use is permitted, offering it as a hosted service to third parties is not, and every
0.x version changes to Apache-2.0 on 2030-12-31.
