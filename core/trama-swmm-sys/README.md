# trama-swmm-sys

EPA SWMM 5.2.4, vendored and statically linked, with the handful of bindings `trama-swmm` needs.
It exists because no `swmm-sys` did.

The C under `SWMM/` is the EPA's, verbatim and in the public domain, and carries no header of
ours. The Rust around it is TRAMA's, under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).

Building it needs a C compiler. Part of [TRAMA](https://trama.build).
