// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A container back into the electrical quantities a power flow needs.
//!
//! Every formula here is pandapower's, checked against the matrix pandapower itself builds rather
//! than against its documentation: line and transformer impedances agree with `ppc["branch"]` to
//! twelve digits on the network in `tests/`. That is deliberate. An importer that is nearly right
//! produces a load flow that converges on the wrong answer, which is worse than one that fails.
//!
//! Where a choice existed, this follows what pandapower does by default rather than what is
//! textbook, because agreeing with the reference implementation is the property under test:
//! transformers are modelled in T and converted to π, and voltage angles are calculated, so a
//! Dyn5's 150° shift is carried rather than dropped.

use std::collections::BTreeMap;

use serde_json::Value;
use trama_format::{edge_properties, node_properties, parse_graph, read_sections};

use crate::flow::{self, Branch, Bus, BusKind, C};
use crate::{MEDIA_TYPE, OWNER};

/// What a model is built to answer, which changes what the network is made of.
///
/// The same file becomes two different electrical networks. A load flow wants the whole π —
/// charging current, iron losses, the phase shift a Dyn imposes — and the demand at every bus.
/// IEC 60909 wants none of that: a short circuit is decided by the impedances between the fault
/// and the sources, so capacitance and load are dropped, the phase shift is irrelevant to a
/// magnitude, and the transformer impedance carries a correction factor the standard prescribes.
#[derive(Clone, Copy)]
pub enum Study {
    /// A load flow, with every load and static generator scaled by this factor.
    Flow { scaling: f64 },
    /// The largest initial symmetrical short-circuit current, IEC 60909 with the voltage factor
    /// `c_max` — 1.1 above 1 kV, which is what a distribution network is.
    Fault { c_max: f64 },
}

/// What a branch's loading is measured against.
pub enum Rating {
    /// A line, rated on current: `max_i_ka` derated by `df` and multiplied by parallel circuits.
    Current { max_ka: f64 },
    /// A transformer, rated on apparent power against its nameplate.
    Power { sn_mva: f64 },
    /// The source declared no limit. Nothing is written to the loading channel for this branch:
    /// a percentage of an unknown rating is not a number, and a NaN delta would poison the
    /// colour ramp of every other branch on the map.
    Unrated,
}

pub struct Model {
    pub buses: Vec<Bus>,
    pub branches: Vec<Branch>,
    /// The container's stable id for each bus. Auxiliary buses — the open side of an open switch —
    /// carry none, because they exist in the calculation and not in the network.
    pub bus_entity: Vec<Option<u64>>,
    pub branch_entity: Vec<u64>,
    pub rating: Vec<Rating>,
    /// Nominal voltage per bus, kV, for turning per-unit currents back into kA.
    pub base_kv: Vec<f64>,
    pub sn_mva: f64,
    /// Admittances to earth, by bus: the source impedance of each external grid under a fault.
    /// Empty for a load flow, where the same infeed is a slack instead.
    pub shunts: Vec<(usize, C)>,
}

/// Read a container compiled from a pandapower network into the network the study needs.
pub fn model(container: &[u8], study: Study) -> Result<Model, String> {
    let document = remainder(container)?;
    let tables = document.get("_object").and_then(Value::as_object).ok_or("the XTRA record is not a pandapower net")?;
    let scalar = |name: &str, fallback: f64| tables.get(name).and_then(Value::as_f64).unwrap_or(fallback);
    let sn_mva = scalar("sn_mva", 1.0);
    let f_hz = scalar("f_hz", 50.0);

    let graph = parse_graph(
        &read_sections(container)?
            .into_iter()
            .find(|section| &section.kind == b"GRPH")
            .ok_or("container is missing a GRPH section")?
            .payload,
    )?;
    let nodes = node_properties(container)?;
    let edges = edge_properties(container)?;

    // pandapower addresses everything by its own index, so the first job is to be able to go from
    // one to a position in the graph. Both directions are needed: rows reference buses by index,
    // and deltas are written against the container's stable ids.
    let mut by_index: BTreeMap<i64, usize> = BTreeMap::new();
    let mut buses = Vec::with_capacity(graph.nodes.len());
    let mut bus_entity = Vec::with_capacity(graph.nodes.len());
    let mut base_kv = Vec::with_capacity(graph.nodes.len());
    for (position, node) in graph.nodes.iter().enumerate() {
        let row = nodes.get(node.property_row as usize).ok_or("a node references a property row that is not there")?;
        let index = integer(row, "power:index").ok_or("a node carries no 'power:index'; it is not a bus")?;
        by_index.insert(index, position);
        base_kv.push(number(row, "power:vn_kv").ok_or_else(|| format!("bus {index} declares no nominal voltage"))?);
        bus_entity.push(Some(node.id));
        buses.push(Bus { kind: BusKind::Load, p_pu: 0.0, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 });
    }

    // An open switch does not delete its branch: pandapower moves that end onto a bus of its own,
    // so the branch still charges its capacitance from the side that stays connected and carries
    // no through current. Reproducing that is what makes those branches' loadings match.
    let mut open: BTreeMap<(&str, i64), Vec<i64>> = BTreeMap::new();
    for row in rows(tables, "switch")? {
        let closed = row.get("closed").and_then(Value::as_bool).unwrap_or(true);
        let (Some(element), Some(bus)) =
            (row.get("element").and_then(Value::as_i64), row.get("bus").and_then(Value::as_i64))
        else {
            continue;
        };
        let kind = match row.get("et").and_then(Value::as_str) {
            Some("l") => "line",
            Some("t") => "trafo",
            // A bus-to-bus switch joins or separates two buses rather than detaching an element.
            // Closed ones are the common case and are already one node in the graph; an open one
            // is not modelled, so say so rather than solve a network that is not the one asked for.
            Some("b") if !closed => {
                return Err("this network has an open bus-bus switch, which this solver does not model".into());
            }
            _ => continue,
        };
        if !closed {
            open.entry((kind, element)).or_default().push(bus);
        }
    }

    let mut branches = Vec::with_capacity(graph.edges.len());
    let mut branch_entity = Vec::with_capacity(graph.edges.len());
    let mut rating = Vec::new();
    for edge in &graph.edges {
        let row = edges.get(edge.property_row as usize).ok_or("an edge references a property row that is not there")?;
        let kind = text(row, "power:kind").ok_or("an edge carries no 'power:kind'")?;
        let index = integer(row, "power:index").ok_or("an edge carries no 'power:index'")?;
        if !boolean(row, "power:in_service") {
            continue;
        }
        let (from_column, to_column) = match kind.as_str() {
            "line" => ("power:from_bus", "power:to_bus"),
            "trafo" => ("power:hv_bus", "power:lv_bus"),
            other => return Err(format!("edge {} is a '{other}', which this solver does not know", edge.id)),
        };
        let ends = [from_column, to_column].map(|column| integer(row, column));
        let (Some(from_index), Some(to_index)) = (ends[0], ends[1]) else {
            return Err(format!("{kind} {index} does not name both of its buses"));
        };
        let mut from = *by_index
            .get(&from_index)
            .ok_or_else(|| format!("{kind} {index} names bus {from_index}, which is not in this container"))?;
        let mut to = *by_index
            .get(&to_index)
            .ok_or_else(|| format!("{kind} {index} names bus {to_index}, which is not in this container"))?;

        // Detach whichever ends carry an open switch onto a bus of their own.
        for detached in open.get(&(kind.as_str(), index)).map(Vec::as_slice).unwrap_or_default() {
            let end = if *detached == from_index { &mut from } else { &mut to };
            let auxiliary = buses.len();
            buses.push(Bus { kind: BusKind::Load, p_pu: 0.0, q_pu: 0.0, vm_pu: 1.0, va_rad: 0.0 });
            bus_entity.push(None);
            base_kv.push(base_kv[*end]);
            *end = auxiliary;
        }

        let (branch, rated) = match kind.as_str() {
            "line" => line(row, from, to, base_kv[from], sn_mva, f_hz, study)?,
            _ => transformer(row, from, to, base_kv[from], base_kv[to], sn_mva, study)?,
        };
        branches.push(branch);
        branch_entity.push(edge.id);
        rating.push(rated);
    }

    // Injections. A load consumes and a static generator produces, both scaled by their own column
    // and by the caller's factor; the two differ only in sign.
    //
    // A fault has none of this. IEC 60909 computes the current from the sources through the
    // network impedance with the pre-fault load ignored, because the fault current dwarfs it and
    // the standard's whole point is an answer that does not depend on the operating state.
    let scaling = match study {
        Study::Flow { scaling } => scaling,
        Study::Fault { .. } => 0.0,
    };
    for (table, sign) in [("load", -1.0), ("sgen", 1.0)] {
        for row in rows(tables, table)? {
            if !row.get("in_service").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(position) = row.get("bus").and_then(Value::as_i64).and_then(|bus| by_index.get(&bus)) else {
                continue;
            };
            let factor = sign * row.get("scaling").and_then(Value::as_f64).unwrap_or(1.0) * scaling;
            buses[*position].p_pu += factor * row.get("p_mw").and_then(Value::as_f64).unwrap_or(0.0) / sn_mva;
            buses[*position].q_pu += factor * row.get("q_mvar").and_then(Value::as_f64).unwrap_or(0.0) / sn_mva;
        }
    }

    // The external grid holds the voltage. Its own scaling is none: a slack absorbs whatever the
    // network needs, which is what makes it the slack.
    //
    // Under a fault it is not a slack at all but a source impedance to earth: what the rest of the
    // system upstream can deliver, condensed into `s_sc_max_mva` and an R/X ratio. That impedance
    // is most of the answer at the head of a feeder, and all of it at the infeed itself.
    let mut slacks = 0;
    let mut shunts: Vec<(usize, C)> = Vec::new();
    for row in rows(tables, "ext_grid")? {
        if !row.get("in_service").and_then(Value::as_bool).unwrap_or(true) {
            continue;
        }
        let Some(position) = row.get("bus").and_then(Value::as_i64).and_then(|bus| by_index.get(&bus)) else {
            continue;
        };
        buses[*position].kind = BusKind::Slack;
        buses[*position].vm_pu = row.get("vm_pu").and_then(Value::as_f64).unwrap_or(1.0);
        buses[*position].va_rad = row.get("va_degree").and_then(Value::as_f64).unwrap_or(0.0).to_radians();
        slacks += 1;
        if let Study::Fault { c_max } = study {
            let power = row.get("s_sc_max_mva").and_then(Value::as_f64).filter(|value| *value > 0.0).ok_or(
                "an external grid declares no 's_sc_max_mva', so there is nothing to say how much \
                 fault current the system upstream can deliver",
            )?;
            let magnitude = c_max * sn_mva / power;
            let ratio = row.get("rx_max").and_then(Value::as_f64).unwrap_or(0.1);
            let reactance = magnitude / (1.0 + ratio * ratio).sqrt();
            shunts.push((*position, C::new(ratio * reactance, reactance).inv()));
        }
    }
    if slacks == 0 {
        return Err("this network has no external grid in service, so no bus holds a voltage".into());
    }

    Ok(Model { buses, branches, bus_entity, branch_entity, rating, base_kv, sn_mva, shunts })
}

/// A line as a π section, per-unit on the bus it runs between.
fn line(
    row: &BTreeMap<String, Value>,
    from: usize,
    to: usize,
    base_kv: f64,
    sn_mva: f64,
    f_hz: f64,
    study: Study,
) -> Result<(Branch, Rating), String> {
    let value = |key: &str, fallback: f64| number(row, key).unwrap_or(fallback);
    let length = value("power:length_km", 0.0);
    let parallel = value("power:parallel", 1.0).max(1.0);
    let base_ohm = base_kv * base_kv / sn_mva;
    let r = value("power:r_ohm_per_km", 0.0) * length / parallel / base_ohm;
    let x = value("power:x_ohm_per_km", 0.0) * length / parallel / base_ohm;
    // Charging: nanofarads per kilometre into a susceptance, and a conductance in microsiemens.
    let b = 2.0 * std::f64::consts::PI * f_hz * value("power:c_nf_per_km", 0.0) * 1e-9 * length * parallel * base_ohm;
    let g = value("power:g_us_per_km", 0.0) * 1e-6 * length * parallel * base_ohm;
    if r == 0.0 && x == 0.0 {
        return Err("a line has no impedance, so nothing limits the current through it".into());
    }
    // A fault is decided by the impedance between it and the sources. Charging current is orders
    // of magnitude below the fault current and IEC 60909 leaves it out, so this is the standard
    // speaking rather than a simplification of ours.
    let half = match study {
        Study::Flow { .. } => C::new(g / 2.0, b / 2.0),
        Study::Fault { .. } => C::ZERO,
    };
    let rating = match number(row, "power:max_i_ka") {
        Some(max_ka) if max_ka > 0.0 => Rating::Current { max_ka: max_ka * value("power:df", 1.0) * parallel },
        _ => Rating::Unrated,
    };
    Ok((
        Branch {
            from,
            to,
            y_series: C::new(r, x).inv(),
            y_shunt_from: half,
            y_shunt_to: half,
            ratio: C::new(1.0, 0.0),
        },
        rating,
    ))
}

/// A two-winding transformer, modelled in T and converted to π exactly as pandapower does.
///
/// The T model puts the magnetising branch between the two leakage impedances rather than at one
/// end, which is the physical arrangement; the star-delta conversion below is what turns it back
/// into the π the admittance matrix wants. Skipping it and using the π directly changes the fifth
/// digit of every impedance — enough that a comparison against the reference stops passing, and
/// not enough that anyone would notice the model was wrong.
fn transformer(
    row: &BTreeMap<String, Value>,
    from: usize,
    to: usize,
    hv_base_kv: f64,
    lv_base_kv: f64,
    sn_mva: f64,
    study: Study,
) -> Result<(Branch, Rating), String> {
    let value = |key: &str, fallback: f64| number(row, key).unwrap_or(fallback);
    let rated_mva = value("power:sn_mva", 0.0);
    if rated_mva <= 0.0 {
        return Err("a transformer declares no rating, so its impedance cannot be referred".into());
    }
    let parallel = value("power:parallel", 1.0).max(1.0);
    let vn_hv = value("power:vn_hv_kv", hv_base_kv);
    let vn_lv = value("power:vn_lv_kv", lv_base_kv);

    // The tap moves the nominal voltage of the side it sits on. `tap_step_degree` would also swing
    // the angle; it is absent on a ratio changer, which is what a distribution transformer has.
    let steps =
        value("power:tap_step_percent", 0.0) * (value("power:tap_pos", 0.0) - value("power:tap_neutral", 0.0)) / 100.0;
    let angle = value("power:tap_step_degree", 0.0).to_radians();
    let tapped = |nominal: f64| {
        let delta = nominal * steps;
        ((nominal + delta * angle.cos()).powi(2) + (delta * angle.sin()).powi(2)).sqrt()
    };
    let tap_shift = |nominal: f64, direction: f64| {
        let delta = nominal * steps;
        (direction * delta * angle.sin()).atan2(nominal + delta * angle.cos())
    };
    let side = text(row, "power:tap_side").unwrap_or_default();
    let (vn_hv_tapped, vn_lv_tapped) = match side.as_str() {
        "hv" => (tapped(vn_hv), vn_lv),
        "lv" => (vn_hv, tapped(vn_lv)),
        _ => (vn_hv, vn_lv),
    };
    let shift = value("power:shift_degree", 0.0).to_radians()
        + match side.as_str() {
            "hv" => tap_shift(vn_hv, 1.0),
            "lv" => tap_shift(vn_lv, -1.0),
            _ => 0.0,
        };

    // Short-circuit impedance, referred to the system base through the low side.
    let referred = (vn_lv_tapped / lv_base_kv).powi(2) * sn_mva;
    let z = value("power:vk_percent", 0.0) / 100.0 / rated_mva * referred;
    let r = value("power:vkr_percent", 0.0) / 100.0 / rated_mva * referred;
    if z.abs() <= r.abs() {
        return Err("a transformer's short-circuit voltage is not above its resistive part".into());
    }
    let x = z.signum() * (z * z - r * r).sqrt();
    let (r, x) = (r / parallel, x / parallel);

    // IEC 60909 §3.7 corrects a transformer's impedance by a factor that depends on its own
    // reactance, because the standard's fault current is computed at a voltage the network is not
    // actually at. Without it every current downstream of a transformer is a couple of percent out
    // — small enough to look like rounding, large enough to size a breaker wrongly.
    let (r, x) = match study {
        Study::Flow { .. } => (r, x),
        Study::Fault { c_max } => {
            // The reactance relative to the transformer's own rating, which is what §3.7 asks for.
            let relative = x * rated_mva / ((vn_lv_tapped / lv_base_kv).powi(2) * sn_mva);
            let correction = 0.95 * c_max / (1.0 + 0.6 * relative);
            (r * correction, x * correction)
        }
    };

    // Magnetising branch: iron losses give the conductance, no-load current the susceptance.
    let base_ohm = lv_base_kv * lv_base_kv / sn_mva;
    let referral = (vn_lv_tapped / vn_lv).powi(2);
    let iron_mw = value("power:pfe_kw", 0.0) * 1e-3;
    let no_load_mva = value("power:i0_percent", 0.0) / 100.0 * rated_mva;
    let susceptance_mva = -(no_load_mva * no_load_mva - iron_mw * iron_mw).max(0.0).sqrt();
    let scale = base_ohm * parallel / (vn_lv * vn_lv) / referral;
    let magnetising = C::new(iron_mw * scale, susceptance_mva * scale);

    // Star to delta: the leakage splits evenly either side of the magnetising branch, which is
    // pandapower's default and leaves the resulting π symmetric. A fault sees no magnetising
    // branch at all — it is a shunt across a source, and the standard drops it.
    let magnetising = match study {
        Study::Flow { .. } => magnetising,
        Study::Fault { .. } => C::ZERO,
    };
    let (series, shunt) = if magnetising == C::ZERO {
        (C::new(r, x).inv(), C::ZERO)
    } else {
        let half = C::new(r / 2.0, x / 2.0);
        let centre = magnetising.inv();
        let total = half * half + half * centre + half * centre;
        // ×2 because each end carries half of what the branch declares.
        (total.inv() * centre, (total / half).inv() * C::new(2.0, 0.0))
    };
    let half_shunt = shunt / C::new(2.0, 0.0);

    let ratio = (vn_hv_tapped / vn_lv_tapped) / (hv_base_kv / lv_base_kv);
    // The shift rotates every voltage downstream of it by the same angle, so it cannot change the
    // magnitude of a fault current. Dropping it keeps this model the one IEC 60909 describes.
    let shift = match study {
        Study::Flow { .. } => shift,
        Study::Fault { .. } => 0.0,
    };
    Ok((
        Branch {
            from,
            to,
            y_series: series,
            y_shunt_from: half_shunt,
            y_shunt_to: half_shunt,
            ratio: C::polar(ratio, shift),
        },
        Rating::Power { sn_mva: rated_mva * parallel },
    ))
}

/// Loading in percent for each branch, and `None` where the source declared no rating.
pub fn loadings(model: &Model, solution: &flow::Solution) -> Vec<Option<f64>> {
    let base_current = |bus: usize| model.sn_mva / (model.base_kv[bus] * 3f64.sqrt());
    flow::branch_flows(&model.branches, solution)
        .iter()
        .zip(&model.rating)
        .enumerate()
        .map(|(index, (flow, rating))| match rating {
            Rating::Unrated => None,
            Rating::Current { max_ka } => {
                let branch = &model.branches[index];
                let from_ka = flow.current_from.abs() * base_current(branch.from);
                let to_ka = flow.current_to.abs() * base_current(branch.to);
                Some(from_ka.max(to_ka) / max_ka * 100.0)
            }
            Rating::Power { sn_mva } => {
                let larger = (flow.power_from.abs()).max(flow.power_to.abs()) * model.sn_mva;
                Some(larger / sn_mva * 100.0)
            }
        })
        .collect()
}

/// The initial symmetrical short-circuit current at each bus, in kA.
///
/// IEC 60909's `Ikss = c·Un / (√3·|Zk|)`, which in per-unit is `c / |Z_ii|` scaled by the bus's
/// own base current. It is the number that sizes a breaker and sets what a protection relay must
/// survive, and the reason a utility runs the study at all.
pub fn fault_currents(model: &Model, c_max: f64) -> Result<Vec<f64>, flow::Failure> {
    let impedance = flow::thevenin(model.buses.len(), &model.branches, &model.shunts)?;
    Ok(impedance
        .iter()
        .enumerate()
        .map(|(bus, z)| {
            let base_current = model.sn_mva / (model.base_kv[bus] * 3f64.sqrt());
            c_max * base_current / z.abs()
        })
        .collect())
}

/// The pandapower document the compiler carried without reading it.
fn remainder(container: &[u8]) -> Result<Value, String> {
    for section in read_sections(container)? {
        if &section.kind != b"XTRA" {
            continue;
        }
        let at = |offset: usize| u32::from_le_bytes(section.payload[offset..offset + 4].try_into().unwrap()) as usize;
        let owner = String::from_utf8_lossy(&section.payload[at(0)..at(0) + at(4)]).into_owned();
        let media_type = String::from_utf8_lossy(&section.payload[at(8)..at(8) + at(12)]).into_owned();
        if owner == OWNER && media_type == MEDIA_TYPE {
            let text = String::from_utf8_lossy(&section.payload[at(16)..at(16) + at(20)]).into_owned();
            return serde_json::from_str(&text).map_err(|error| format!("the pandapower record is not JSON: {error}"));
        }
    }
    Err("this container was not compiled from a pandapower network".into())
}

/// One table's rows keyed by column name. pandas writes a frame as a JSON string inside the JSON.
fn rows(tables: &serde_json::Map<String, Value>, name: &str) -> Result<Vec<BTreeMap<String, Value>>, String> {
    let Some(encoded) = tables.get(name).and_then(|table| table.get("_object")).and_then(Value::as_str) else {
        // A network with no static generation has no `sgen` table, which is not an error.
        return Ok(Vec::new());
    };
    let parsed: Value =
        serde_json::from_str(encoded).map_err(|error| format!("the '{name}' table is not JSON: {error}"))?;
    let columns: Vec<String> = parsed["columns"]
        .as_array()
        .map(|list| list.iter().map(|v| v.as_str().unwrap_or_default().to_string()).collect())
        .unwrap_or_default();
    Ok(parsed["data"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|row| {
                    columns
                        .iter()
                        .cloned()
                        .zip(row.as_array().cloned().unwrap_or_default())
                        .filter(|(_, value)| !value.is_null())
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default())
}

fn number(row: &BTreeMap<String, Value>, key: &str) -> Option<f64> {
    row.get(key).and_then(Value::as_f64)
}

fn integer(row: &BTreeMap<String, Value>, key: &str) -> Option<i64> {
    row.get(key).and_then(Value::as_i64)
}

fn text(row: &BTreeMap<String, Value>, key: &str) -> Option<String> {
    row.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Absent means in service: pandapower's own default, and the column is dropped when it is null.
fn boolean(row: &BTreeMap<String, Value>, key: &str) -> bool {
    row.get(key).and_then(Value::as_bool).unwrap_or(true)
}
