// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! EPANET `.inp` import and export, following `docs/EPANET_BOUNDARY.md`.
//!
//! Every hydraulic concept in the project lives in this crate. `trama-format` knows nodes,
//! edges, typed properties and opaque records, and never learns what a pump is.

pub mod exporter;
pub mod importer;
pub mod inp;
