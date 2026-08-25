# trama-solver

The TRAMA solver contract, as a runtime. A solver reads a container's graph and writes state back
as packed 18-byte deltas — `(entity_id: u64, channel: u16, t: f32, value: f32)` — which is all the
engine ever consumes, so it cannot tell a solver running locally from one across a network.

This crate carries `pack`, the channel resolution a solver needs to find the id it is allowed to
write, the `solver.toml` manifest check, and the HTTP + Server-Sent Events server the reference
solvers are built on.

The contract is specified in
[`docs/SOLVER_CONTRACT.md`](https://github.com/Neixen-labs/trama/blob/main/docs/SOLVER_CONTRACT.md).

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
