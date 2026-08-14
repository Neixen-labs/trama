fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let container = std::fs::read(&arguments[1]).unwrap();
    let deltas = trama_epanet::solver::solve(&container, "pressure", "flow", "age", 0.0, 86400.0)
        .unwrap_or_else(|e| panic!("{e}"));
    // entity, channel, t, value — printed so another implementation can be compared to it.
    for record in deltas.chunks(18) {
        println!(
            "{} {} {} {}",
            u64::from_le_bytes(record[0..8].try_into().unwrap()),
            u16::from_le_bytes(record[8..10].try_into().unwrap()),
            f32::from_le_bytes(record[10..14].try_into().unwrap()),
            f32::from_le_bytes(record[14..18].try_into().unwrap())
        );
    }
}
