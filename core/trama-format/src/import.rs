// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The seam through which a source format the core does not know reaches the compiler.
//!
//! A format that carries domain meaning is read by the crate that owns that domain, never by
//! this one. An importer hands back GeoJSON features, which the compiler already speaks, plus
//! whatever the format would otherwise lose as opaque records.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::Extra;

/// What an importer produces: things the format can express, and things it cannot.
pub struct Import {
    pub features: Vec<Value>,
    /// No default: an importer that loses nothing should have to say so, since the alternative
    /// is dropping a pattern or a curve without ever noticing.
    pub extras: Vec<Extra>,
    /// What a container built from this format can be solved for. STCH is a declaration, never
    /// data, and which channels a format implies is the importer's knowledge, not the caller's.
    pub channels: Vec<Value>,
}

/// Reads one family of source files. Implementations live outside this crate.
pub trait Importer {
    fn suffixes(&self) -> &'static [&'static str];
    fn load(&self, source: &Path, options: &BTreeMap<String, String>) -> Result<Import, String>;
}

/// Turn `-o key=value` into a mapping, rejecting the malformed before any work starts.
pub fn parse_options(pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    pairs
        .iter()
        .map(|pair| match pair.split_once('=') {
            Some((key, value)) if !key.is_empty() => Ok((key.to_string(), value.to_string())),
            _ => Err(format!("option '{pair}' is not key=value")),
        })
        .collect()
}
