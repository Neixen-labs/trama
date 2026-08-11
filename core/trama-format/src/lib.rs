// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The TRAMA v0 container.
//!
//! Ported from the Python compiler and kept byte-identical to it while both exist. Every
//! ordering, rounding and string form here matches that implementation on purpose; where the
//! two could disagree, the tests compare their bytes rather than trusting either.

mod export;
mod import;
mod read;
mod write;

pub use export::{Export, export};
pub use import::{Import, Importer, parse_options};
pub use read::{Edge, GeometryReference, Graph, Node, Section, parse_graph, read_sections};
pub use write::{Extra, compile};

/// CRC-32C (Castagnoli), the checksum every section carries.
pub(crate) fn crc32c(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0x82F6_3B78 } else { 0 };
        }
    }
    !crc
}
