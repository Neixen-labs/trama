// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Links EPA SWMM 5.2.4. The crate exports no bindings on purpose: the dozen functions the
//! solver calls are declared where they are used, the way `trama-epanet` treats `epanet-sys`,
//! because bindgen against a WASI target produces constants and no functions and the browser
//! build needs that target. Depending on this crate is what puts `libswmm5` on the link line.
//!
//! `SWMM/` is the EPA's source, verbatim: public domain, from
//! <https://github.com/USEPA/Stormwater-Management-Model> tag `v5.2.4`, `src/solver` only.
