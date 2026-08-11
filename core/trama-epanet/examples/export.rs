fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let container = std::fs::read(&arguments[1]).unwrap();
    let text = trama_epanet::exporter::export_inp(&container, &arguments[3]).unwrap_or_else(|e| panic!("{e}"));
    std::fs::write(&arguments[2], text).unwrap();
}
