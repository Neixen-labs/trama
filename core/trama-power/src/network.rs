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

use serde_json::{Value, json};
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
    /// A transformer, rated on the current each winding can carry at its own nominal voltage —
    /// `i·vn·√3` against the nameplate, taken at whichever end is worse. Not the apparent power at
    /// the terminals, which is the other thing pandapower can be asked for and is not its default:
    /// the two agree only where the bus sits at the winding's nominal voltage, so a network solved
    /// away from nominal reports one number under the first rule and a different one under the
    /// second. A winding burns on current.
    ///
    /// An end is `None` where there is no winding to measure. That is the star point of a
    /// three-winding transformer: the equivalent branch reaching it has one real winding and one
    /// end inside the machine, and taking the worse of the two would charge that branch the series
    /// losses of the other end as if a coil somewhere were carrying them.
    Winding { sn_mva: f64, vn_from_kv: Option<f64>, vn_to_kv: Option<f64> },
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
        buses.push(Bus::floating());
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

    // Each expressed table's columns travel in the record even though its rows do not, which is
    // the only thing that can tell a column nobody filled in from one that never existed.
    let declares_changer = |table: &str| {
        tables
            .get(table)
            .and_then(|frame| frame.get("_object"))
            .and_then(Value::as_str)
            .and_then(|encoded| serde_json::from_str::<Value>(encoded).ok())
            .and_then(|parsed| parsed["columns"].as_array().cloned())
            .is_some_and(|columns| columns.iter().any(|column| column == "tap_changer_type"))
    };

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
        // Which two buses an edge runs between. A branch of a three-winding transformer has one
        // end on a bus of the network and the other on the star point the importer gave it, which
        // carries the negative of the transformer's own index and no row of its own.
        let side = text(row, "power:side").unwrap_or_default();
        let ends = match kind.as_str() {
            "line" => (integer(row, "power:from_bus"), integer(row, "power:to_bus")),
            "trafo" => (integer(row, "power:hv_bus"), integer(row, "power:lv_bus")),
            "trafo3w" => match side.as_str() {
                "hv" => (integer(row, "power:hv_bus"), Some(-(index + 1))),
                "mv" | "lv" => (Some(-(index + 1)), integer(row, &format!("power:{side}_bus"))),
                other => return Err(format!("trafo3w {index} carries side '{other}'")),
            },
            other => return Err(format!("edge {} is a '{other}', which this solver does not know", edge.id)),
        };
        let (Some(from_index), Some(to_index)) = ends else {
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
            buses.push(Bus::floating());
            bus_entity.push(None);
            base_kv.push(base_kv[*end]);
            *end = auxiliary;
        }

        // A tap changer counts where the file's own schema has somewhere to name one and this row
        // did. A network written before pandapower 3.0 has no such column at all, and there a tap
        // position is all there is to go on — so absence of the column means the old reading and
        // absence of the value under it means no changer.
        let terms = Terms {
            sn_mva,
            f_hz,
            study,
            correction: Correction::Apply,
            regulated: !declares_changer(&kind) || row.contains_key("power:tap_changer_type"),
        };
        let (branch, rated) = match kind.as_str() {
            "line" => line(row, from, to, base_kv[from], terms)?,
            "trafo3w" => transformer3w(row, &side, from, to, base_kv[from], base_kv[to], terms)?,
            _ => transformer(row, from, to, base_kv[from], base_kv[to], terms)?,
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

    // A shunt is a capacitor bank or a reactor: an admittance to earth, and the cheapest reactive
    // power on a network. `q_mvar` is what it *consumes* at nominal voltage, so a capacitor
    // declares it negative and lands as a positive susceptance. Ignoring the table would drop the
    // bank from the model and report a voltage the network only reaches because of it.
    //
    // Under a fault the standard drops it with everything else capacitive: IEC 60909 asks for the
    // impedance between the fault and the sources, and a bank across a bus is neither.
    if matches!(study, Study::Flow { .. }) {
        for row in rows(tables, "shunt")? {
            if !row.get("in_service").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(position) = row.get("bus").and_then(Value::as_i64).and_then(|bus| by_index.get(&bus)) else {
                continue;
            };
            // A bank rated for another voltage delivers what the square of the ratio says it does.
            let rated_kv = row.get("vn_kv").and_then(Value::as_f64).unwrap_or(base_kv[*position]);
            let ratio = (base_kv[*position] / rated_kv).powi(2);
            let steps = row.get("step").and_then(Value::as_f64).unwrap_or(1.0);
            let scale = ratio * steps / sn_mva;
            let p_mw = row.get("p_mw").and_then(Value::as_f64).unwrap_or(0.0);
            let q_mvar = row.get("q_mvar").and_then(Value::as_f64).unwrap_or(0.0);
            buses[*position].y_shunt = buses[*position].y_shunt + C::new(p_mw * scale, -q_mvar * scale);
        }
    }

    // A synchronous generator on automatic voltage regulation: real power despatched, voltage
    // held, reactive power whatever holding it costs — up to the limits the machine declares.
    //
    // Its real power carries no study scaling, unlike a load or a static generator. Those two are
    // demand and weather; this is a despatch decision, and scaling it would mean answering what
    // the network does at 120% load with a station that also happens to be at 120% output.
    //
    // Under a fault it is neither slack nor load but a source: a synchronous machine feeds a short
    // circuit from behind its subtransient reactance, and it is the second largest contribution on
    // any network that has one. See [`generator`] for the impedance and the correction §3.6 puts
    // on it.
    if let Study::Fault { c_max } = study {
        for row in rows(tables, "gen")? {
            if !row.get("in_service").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(position) = row.get("bus").and_then(Value::as_i64).and_then(|bus| by_index.get(&bus)) else {
                continue;
            };
            buses[*position].y_shunt = buses[*position].y_shunt + generator(&row, base_kv[*position], sn_mva, c_max)?;
        }
    }
    if matches!(study, Study::Flow { .. }) {
        for row in rows(tables, "gen")? {
            if !row.get("in_service").and_then(Value::as_bool).unwrap_or(true) {
                continue;
            }
            let Some(position) = row.get("bus").and_then(Value::as_i64).and_then(|bus| by_index.get(&bus)) else {
                continue;
            };
            let scaling = row.get("scaling").and_then(Value::as_f64).unwrap_or(1.0);
            buses[*position].p_pu += scaling * row.get("p_mw").and_then(Value::as_f64).unwrap_or(0.0) / sn_mva;
            buses[*position].vm_pu = row.get("vm_pu").and_then(Value::as_f64).unwrap_or(1.0);
            // A machine that declares no limit is one whose file never recorded it, not one that
            // can deliver without bound. Which of those to assume is the caller's risk to take.
            let limit = |key: &str, absent: f64| {
                row.get(key).and_then(Value::as_f64).map(|mvar| mvar / sn_mva).unwrap_or(absent)
            };
            let (q_min_pu, q_max_pu) = (limit("min_q_mvar", f64::NEG_INFINITY), limit("max_q_mvar", f64::INFINITY));
            // Two machines on one bus hold one voltage between them and their limits add. The
            // second setpoint silently wins above, which is what pandapower does with the same
            // input; a file that disagrees with itself about a bus's voltage is not modelled here.
            buses[*position].kind = match buses[*position].kind {
                BusKind::Voltage { q_min_pu: min, q_max_pu: max } => {
                    BusKind::Voltage { q_min_pu: min + q_min_pu, q_max_pu: max + q_max_pu }
                }
                _ => BusKind::Voltage { q_min_pu, q_max_pu },
            };
        }
    }

    // The external grid holds the voltage. Its own scaling is none: a slack absorbs whatever the
    // network needs, which is what makes it the slack.
    //
    // Under a fault it is not a slack at all but a source impedance to earth: what the rest of the
    // system upstream can deliver, condensed into `s_sc_max_mva` and an R/X ratio. That impedance
    // is most of the answer at the head of a feeder, and all of it at the infeed itself.
    let mut slacks = 0;
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
            buses[*position].y_shunt = buses[*position].y_shunt + C::new(ratio * reactance, reactance).inv();
        }
    }
    if slacks == 0 {
        return Err("this network has no external grid in service, so no bus holds a voltage".into());
    }

    Ok(Model { buses, branches, bus_entity, branch_entity, rating, base_kv, sn_mva })
}

/// A line as a π section, per-unit on the bus it runs between.
fn line(
    row: &BTreeMap<String, Value>,
    from: usize,
    to: usize,
    base_kv: f64,
    terms: Terms,
) -> Result<(Branch, Rating), String> {
    let Terms { sn_mva, f_hz, study, .. } = terms;
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

/// What a branch needs that its own row does not carry.
///
/// `regulated` is the one that is not obvious: pandapower moves a transformer's ratio only where
/// the file names a tap changer in `tap_changer_type`, and a `tap_pos` beside an empty one is a
/// column nobody filled in rather than a transformer on tap three. Reading the position alone
/// moves a voltage the reference does not move — by a percent or two, in whichever direction the
/// position happens to sit.
#[derive(Clone, Copy)]
struct Terms {
    sn_mva: f64,
    f_hz: f64,
    study: Study,
    correction: Correction,
    regulated: bool,
}

/// Whether a transformer's short-circuit voltage still needs IEC 60909 §3.7's correction.
///
/// A three-winding transformer's three branches carry it already. The standard corrects the
/// impedance of a *pair* of windings, which is what the nameplate declares and what the star split
/// consumes, so the factor has to go on before the split rather than on the three branches that
/// come out of it — applying it twice, or to the wrong quantity, moves every downstream fault
/// current by a couple of percent and nothing says so.
#[derive(Clone, Copy, PartialEq)]
enum Correction {
    Apply,
    Already,
}

/// One branch of a three-winding transformer: the equivalent two-winding one pandapower solves.
///
/// This is `_trafo_df_from_trafo3w` in pandapower's `build_branch.py`, one row at a time. It builds
/// a two-winding transformer table out of the three-winding one and hands it to the very code that
/// handles ordinary transformers, and so does this — the equivalent's nameplate is assembled here
/// and [`transformer`] does the physics, which keeps the two kinds from drifting apart.
///
/// The impedances are not a third each. A three-winding transformer's nameplate gives the
/// short-circuit voltage of each *pair* of windings, measured with the third open, each on the
/// smaller rating of its own pair. Referring all three onto the high side's rating and running the
/// delta-star transform is what turns three pairwise measurements into three branch impedances,
/// and one of them is routinely negative — the star point is a calculation, not a place, and the
/// middle winding of a real machine sits close enough to the others that its share comes out below
/// zero. That is not an error to guard against; a negative reactance in one leg is the standard
/// model, and `vk_percent` keeps the sign so [`transformer`] rebuilds it rather than the magnitude.
fn transformer3w(
    row: &BTreeMap<String, Value>,
    side: &str,
    from: usize,
    to: usize,
    hv_base_kv: f64,
    lv_base_kv: f64,
    terms: Terms,
) -> Result<(Branch, Rating), String> {
    // Every quantity below is per winding, so each is read as a triple in `hv, mv, lv` order.
    let three = |prefix: &str, suffix: &str| -> Result<[f64; 3], String> {
        let mut ends = [0.0; 3];
        for (position, end) in ["hv", "mv", "lv"].into_iter().enumerate() {
            let key = format!("power:{prefix}{end}{suffix}");
            ends[position] =
                number(row, &key).ok_or_else(|| format!("a three-winding transformer declares no '{key}'"))?;
        }
        Ok(ends)
    };
    // A tap on the star point re-refers the ratio onto the other end of its own branch and turns
    // the step complex. It is a real arrangement — regulation in the tertiary — and refused rather
    // than approximated, because a wrong ratio is a plausible voltage.
    // ponytail: refused, not modelled. pandapower's `_calculate_3w_tap_changers` has the six lines
    // it takes; what is missing is a fixture that would catch getting them wrong.
    if row.get("power:tap_at_star_point").and_then(Value::as_bool).unwrap_or(false) {
        return Err("a three-winding transformer taps at its star point, which this solver does not model".into());
    }

    let sn = three("sn_", "_mva")?;
    let vk = three("vk_", "_percent")?;
    let vkr = three("vkr_", "_percent")?;
    let vn = three("vn_", "_kv")?;
    if sn.iter().any(|rating| *rating <= 0.0) {
        return Err("a three-winding transformer declares a winding rated at zero".into());
    }

    // Each pair's short-circuit voltage referred onto the high winding's rating. The pairs run
    // hv-mv, mv-lv, hv-lv, and each is measured on the smaller of its own two ratings.
    let pairs = [(0, 1), (1, 2), (0, 2)];
    let referred = |percent: &[f64; 3]| {
        let mut out = [0.0; 3];
        for (leg, (a, b)) in pairs.iter().enumerate() {
            out[leg] = sn[0] * percent[leg] / sn[*a].min(sn[*b]);
        }
        out
    };
    let mut vk_pair = referred(&vk);
    let mut vkr_pair = referred(&vkr);

    // §3.7, on each pair and before the split, with the pair's reactance in its own per-unit.
    if let Study::Fault { c_max } = terms.study {
        for leg in 0..3 {
            let x = ((vk[leg] / 100.0).powi(2) - (vkr[leg] / 100.0).powi(2)).max(0.0).sqrt();
            let kt = 0.95 * c_max / (1.0 + 0.6 * x);
            vk_pair[leg] *= kt;
            vkr_pair[leg] *= kt;
        }
    }

    // Delta to star, on the resistive and reactive parts separately, each branch then re-referred
    // onto its own winding's rating.
    let vki_pair: Vec<f64> =
        (0..3).map(|leg| (vk_pair[leg] * vk_pair[leg] - vkr_pair[leg] * vkr_pair[leg]).max(0.0).sqrt()).collect();
    let star = |pair: &[f64]| {
        [
            0.5 * sn[0] / sn[0] * (pair[0] + pair[2] - pair[1]),
            0.5 * sn[1] / sn[0] * (pair[1] + pair[0] - pair[2]),
            0.5 * sn[2] / sn[0] * (pair[2] + pair[1] - pair[0]),
        ]
    };
    let vkr_branch = star(&vkr_pair);
    let vki_branch = star(&vki_pair);
    let leg = match side {
        "hv" => 0,
        "mv" => 1,
        "lv" => 2,
        other => return Err(format!("a three-winding transformer branch carries side '{other}'")),
    };
    let vk_branch = vki_branch[leg].signum() * (vki_branch[leg].powi(2) + vkr_branch[leg].powi(2)).sqrt();
    if vk_branch == 0.0 {
        return Err("a three-winding transformer splits into a branch with no impedance".into());
    }

    // Iron losses and no-load current belong to one branch only, or the machine would magnetise
    // three times over. pandapower puts them on the side its `trafo3w_losses` option names, which
    // defaults to the high one.
    let losses = text(row, "power:loss_side").unwrap_or_else(|| "hv".into());
    let mine = |key: &str| if losses == side { number(row, key).unwrap_or(0.0) } else { 0.0 };

    let (vn_hv, vn_own) = (vn[0], vn[leg]);
    let mut equivalent: BTreeMap<String, Value> = BTreeMap::new();
    let mut set = |key: &str, value: f64| equivalent.insert(format!("power:{key}"), json!(value));
    set("vk_percent", vk_branch);
    set("vkr_percent", vkr_branch[leg]);
    set("sn_mva", sn[leg]);
    // The high winding's nominal voltage on both sides of the branch that reaches the star point:
    // it steps nothing, which is what makes the star point sit at the high side's base.
    set("vn_hv_kv", vn_hv);
    set("vn_lv_kv", vn_own);
    set("pfe_kw", mine("power:pfe_kw"));
    set("i0_percent", mine("power:i0_percent"));
    set(
        "shift_degree",
        if side == "hv" { 0.0 } else { number(row, &format!("power:shift_{side}_degree")).unwrap_or(0.0) },
    );

    // The tap belongs to whichever branch carries the winding it sits on, and to no other. On the
    // high branch it is a tap on that branch's high side; on the other two the regulated winding is
    // the one away from the star point, so it becomes a tap on the low side.
    if text(row, "power:tap_side").as_deref() == Some(side) {
        equivalent.insert("power:tap_side".into(), json!(if side == "hv" { "hv" } else { "lv" }));
        for key in ["tap_pos", "tap_neutral", "tap_step_percent", "tap_step_degree"] {
            if let Some(value) = number(row, &format!("power:{key}")) {
                equivalent.insert(format!("power:{key}"), json!(value));
            }
        }
    }

    let (branch, _) =
        transformer(&equivalent, from, to, hv_base_kv, lv_base_kv, Terms { correction: Correction::Already, ..terms })?;
    // Only the far end of each branch is a winding. The star point is not.
    let (vn_from_kv, vn_to_kv) = match side {
        "hv" => (Some(vn_own), None),
        _ => (None, Some(vn_own)),
    };
    Ok((branch, Rating::Winding { sn_mva: sn[leg], vn_from_kv, vn_to_kv }))
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
    terms: Terms,
) -> Result<(Branch, Rating), String> {
    let Terms { sn_mva, study, correction, regulated, .. } = terms;
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
    //
    // A fault has no tap at all. IEC 60909 computes the current from the equivalent voltage source
    // through the *nominal* ratio, and the correction factor above is what stands in for the
    // network not being at that voltage — putting the tap back would count the same departure
    // twice. pandapower returns the untapped voltages outright when its mode is `sc`.
    let steps = match regulated && matches!(study, Study::Flow { .. }) {
        true => {
            value("power:tap_step_percent", 0.0) * (value("power:tap_pos", 0.0) - value("power:tap_neutral", 0.0))
                / 100.0
        }
        false => 0.0,
    };
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
        Study::Fault { .. } if correction == Correction::Already => (r, x),
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
        // The nominal voltages here are the windings' own, untapped: a tap changes what the
        // transformer does to the network, not what its windings are built to carry.
        Rating::Winding {
            sn_mva: rated_mva * parallel * value("power:df", 1.0),
            vn_from_kv: Some(vn_hv),
            vn_to_kv: Some(vn_lv),
        },
    ))
}

/// A synchronous machine as a source under a fault: its admittance to earth, per-unit.
///
/// The machine is an EMF behind its subtransient impedance, `Z_G = r_dss + j·x″d·vn²/sn` in ohms
/// at the machine's own rating, and IEC 60909 §3.6 multiplies it by
///
/// ```text
/// K_G = (vn_bus / (vn_gen · (1 + pg%))) · c_max / (1 + x″d · sin φ)
/// ```
///
/// which reconciles two things the standard needs to hold at once: the fault is computed at the
/// equivalent voltage source `c·Un/√3` rather than at the voltage the machine is actually at, and
/// a machine's own rated voltage is usually not its bus's nominal. Both ratios are here rather
/// than assumed to be one, because they only coincide on a machine connected at its own rating —
/// which is the case that would pass a test written from either.
///
/// Every quantity is required and none is defaulted. A machine that declares no `xdss_pu` is a
/// file that never recorded one, and guessing a reactance would put a source of unknown strength
/// on the bus; the study says which column is missing instead. This is the same refusal an
/// external grid without `s_sc_max_mva` already gets, for the same reason.
fn generator(row: &BTreeMap<String, Value>, base_kv: f64, sn_mva: f64, c_max: f64) -> Result<C, String> {
    // A power station unit — machine and its step-up transformer as one — carries a different
    // correction factor, §3.7's `K_S`, computed over the pair rather than the machine. Modelling
    // it as a bare generator would apply the wrong one, so it is refused rather than approximated.
    if row.get("power_station_trafo").is_some_and(|value| !value.is_null()) {
        return Err("a generator declares a power station transformer, which IEC 60909 §3.7 \
                    corrects as one unit rather than as a machine on a bus; this solver does not \
                    model that pairing"
            .into());
    }
    let required = |key: &str| {
        row.get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("a generator declares no '{key}', which a short circuit cannot be computed without"))
    };
    let xdss_pu = required("xdss_pu")?;
    let rated_kv = required("vn_kv")?;
    let rated_mva = required("sn_mva")?;
    let cos_phi = required("cos_phi")?;
    if rated_mva <= 0.0 || rated_kv <= 0.0 {
        return Err("a generator declares a rating of zero, so it has no impedance".into());
    }
    // Absent means the machine runs at its rated voltage, which is what pandapower assumes too.
    let above_rating = row.get("pg_percent").and_then(Value::as_f64).unwrap_or(0.0) / 100.0;

    let reactance_ohm = xdss_pu * rated_kv * rated_kv / rated_mva;
    let resistance_ohm = required("rdss_ohm")?;
    let sin_phi = (1.0 - cos_phi * cos_phi).max(0.0).sqrt();
    let correction = base_kv / (rated_kv * (1.0 + above_rating)) * c_max / (1.0 + xdss_pu * sin_phi);
    // Referred to the bus it sits on, which is the base every other impedance here is already in.
    let base_ohm = base_kv * base_kv / sn_mva;
    let impedance = C::new(resistance_ohm, reactance_ohm) * C::new(correction / base_ohm, 0.0);
    Ok(impedance.inv())
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
            Rating::Winding { sn_mva, vn_from_kv, vn_to_kv } => {
                let branch = &model.branches[index];
                let from = vn_from_kv.map(|kv| flow.current_from.abs() * base_current(branch.from) * kv);
                let to = vn_to_kv.map(|kv| flow.current_to.abs() * base_current(branch.to) * kv);
                let worst = from.into_iter().chain(to).fold(f64::NEG_INFINITY, f64::max);
                Some(worst * 3f64.sqrt() / sn_mva * 100.0)
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
    let impedance = flow::thevenin(&model.buses, &model.branches)?;
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
