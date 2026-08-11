// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A synthetic grid: the benchmark network, and the demo's.
//!
//! Deliberately not a test. It takes seconds and its numbers depend on the machine, so a CI
//! threshold would only produce flakes. Run it when touching anything on the compile path:
//!
//!     cargo run --release -p trama-cli --bin grid -- --side 158 --report

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{Value, json};

const SPACING_DEGREES: f64 = 0.0006; // about 50 m at this latitude
const ORIGIN: (f64, f64) = (-3.75, 40.35);
// The demo's grid: 40x40 nodes at ~150 m spans is 3,120 edges over 20 tiles, enough to show
// tiles meeting without being a file anyone minds cloning.
const DEMO_SPACING_DEGREES: f64 = 0.0018;
const DEMO_ORIGIN: (f64, f64) = (-3.72, 40.39);

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let value = |name: &str| arguments.iter().position(|a| a == name).and_then(|at| arguments.get(at + 1)).cloned();
    let side: usize = value("--side").and_then(|v| v.parse().ok()).unwrap_or(158);
    // The demo's grid declares both, so a solver can colour edges and the ring can hold node
    // rows beside them: `--side 40 --demo --out fixtures/demo-grid.trama` reproduces it.
    let channels: Vec<Value> = if arguments.iter().any(|a| a == "--demo") {
        vec![
            json!({"name": "pressure", "entity_kind": "node", "unit": "m", "min": 0, "max": 80}),
            json!({"name": "flow", "entity_kind": "edge", "unit": "l/s", "min": -50, "max": 50}),
        ]
    } else if arguments.iter().any(|a| a == "--channels") {
        vec![json!({"name": "flow", "entity_kind": "edge", "unit": "1", "min": -50, "max": 50})]
    } else {
        Vec::new()
    };

    let demo = arguments.iter().any(|a| a == "--demo");
    let features = if demo { grid_at(side, DEMO_SPACING_DEGREES, DEMO_ORIGIN) } else { grid(side) };
    let source_bytes =
        serde_json::to_string(&json!({"type": "FeatureCollection", "features": features})).unwrap().len();
    let started = Instant::now();
    let container = trama_format::compile(&features, &channels, &[]).unwrap();
    let elapsed = started.elapsed().as_secs_f64();

    if let Some(path) = value("--out") {
        std::fs::write(&path, &container).unwrap();
        println!("{} edges -> {} ({:.1} MB, {elapsed:.1} s)", features.len(), path, container.len() as f64 / 1e6);
    }
    if arguments.iter().any(|a| a == "--report") {
        // Two references, because "the equivalent GeoJSON" is ambiguous and the answer moves
        // with it. The export is the true equivalent — same entities, same IDs, full precision.
        // The hand-written source omits nodes and rounds coordinates, so it is the harsher one.
        let exported = trama_format::export(&container).unwrap();
        let compact = serde_json::to_string(&exported.nodes).unwrap().len()
            + serde_json::to_string(&exported.edges).unwrap().len();
        println!("{} edges, {:.1} MB source GeoJSON", features.len(), source_bytes as f64 / 1e6);
        println!("compile   {elapsed:6.1} s   {} (criterion < 30 s)", verdict(elapsed < 30.0));
        for (label, reference) in [("equivalent export, compact", compact), ("hand-written source", source_bytes)] {
            let share = container.len() as f64 / reference as f64 * 100.0;
            println!(
                "size      {share:6.1} %   {}  vs {label} ({:.1} MB)",
                verdict(share < 20.0),
                reference as f64 / 1e6
            );
        }
        sections(&container);
    }
}

fn verdict(passed: bool) -> &'static str {
    if passed { "PASS" } else { "FAIL" }
}

/// A side x side node grid wired horizontally and vertically, like a distribution network.
fn grid(side: usize) -> Vec<Value> {
    grid_at(side, SPACING_DEGREES, ORIGIN)
}

fn grid_at(side: usize, spacing: f64, origin: (f64, f64)) -> Vec<Value> {
    // One expression for every endpoint, so a grid of `side * side` nodes is exactly that.
    // The compiler no longer needs it to be — SPEC 4.2 joins on the quantization grid — but a
    // benchmark should measure the network it claims to, not one the compiler had to repair.
    let point = |row: usize, column: usize| (origin.0 + column as f64 * spacing, origin.1 + row as f64 * spacing);
    let edge = |id: String, start: (f64, f64), end: (f64, f64), row: usize, column: usize| {
        json!({
            "type": "Feature",
            "id": id,
            "properties": {
                "label": format!("pipe-{row}-{column}"),
                "diameter": 100 + (column % 7) * 25,
                "loss": 0.5 + (row % 5) as f64 / 10.0,
            },
            "geometry": {"type": "LineString", "coordinates": [[start.0, start.1], [end.0, end.1]]},
        })
    };
    let mut features = Vec::new();
    for row in 0..side {
        for column in 0..side {
            if column + 1 < side {
                features.push(edge(
                    format!("h{row}-{column}"),
                    point(row, column),
                    point(row, column + 1),
                    row,
                    column,
                ));
            }
            if row + 1 < side {
                features.push(edge(
                    format!("v{row}-{column}"),
                    point(row, column),
                    point(row + 1, column),
                    row,
                    column,
                ));
            }
        }
    }
    features
}

fn sections(container: &[u8]) {
    let mut totals: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    let count = u32::from_le_bytes(container[0x20..0x24].try_into().unwrap()) as usize;
    for index in 0..count {
        let record = 64 + index * 64;
        let kind = String::from_utf8_lossy(&container[record..record + 4]).into_owned();
        let stored = u64::from_le_bytes(container[record + 28..record + 36].try_into().unwrap());
        let entry = totals.entry(kind).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += stored;
    }
    let whole: u64 = totals.values().map(|(_count, bytes)| bytes).sum();
    println!("\nsection  count      stored   share");
    for (kind, (count, bytes)) in totals {
        println!("{kind:<8} {count:>5} {bytes:>11}   {:.1}%", bytes as f64 / whole as f64 * 100.0);
    }
}
