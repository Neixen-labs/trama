# trama-power

Electrical networks for TRAMA: a pandapower JSON imports through `trama compile --importer power`,
keeping every electrical parameter as a typed property, and solves two studies from one file — an
AC power flow by Newton-Raphson writing `voltage` (p.u.) and `loading` (%), and the IEC 60909
maximum-case short circuit every protection setting is derived from, writing `fault_current` (kA).

Being Rust with no engine to link, both run in a browser as readily as on a server.

Every formula here is checked against the matrix pandapower itself builds: 179 bus voltages within
1e-8 p.u. and 183 branch loadings within 1e-6 points on `mv_oberrhein`, every fault current within
1e-9 relative on CIGRE's medium-voltage network. Generators hold voltage until the limit they
declare, three-winding transformers become three edges and the star point pandapower keeps to
itself, and a machine bolted to its step-up transformer is corrected as one impedance under
IEC 60909 §3.7 rather than as two adjacent components.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
