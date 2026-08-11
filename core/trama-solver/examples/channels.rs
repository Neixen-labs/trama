fn main() {
    let container = std::fs::read(std::env::args().nth(1).unwrap()).unwrap();
    for channel in trama_solver::channels(&container).unwrap() {
        println!(
            "{} {} {} unit={} [{}, {}] range_present={}",
            channel.id,
            if channel.entity_kind == 1 { "node" } else { "edge" },
            channel.name,
            channel.unit,
            channel.declared_min,
            channel.declared_max,
            channel.range_present
        );
    }
}
