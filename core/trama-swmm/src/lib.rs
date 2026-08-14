// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! SWMM `.inp` import and export: the drainage network as a graph.
//!
//! Every stormwater concept in the project lives in this crate. A SWMM file shares its text
//! shape with EPANET's — bracketed sections, whitespace fields, `;` comments — so the parsing
//! and reprojection come from `trama-epanet`'s domain-free modules. What differs is which
//! sections are the network: junctions, outfalls, storage and dividers are nodes; conduits,
//! pumps, orifices, weirs and outlets are edges. Everything else — subcatchments, rain gages,
//! time series, options — travels in one opaque `XTRA` record, per SPEC 7: hydrology is input
//! to a simulation, not a property of an entity.

pub mod exporter;
pub mod importer;
#[cfg(feature = "solver")]
pub mod solver;
