fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let data = std::fs::read(&arguments[1]).unwrap();
    let exported = trama_format::export(&data).unwrap();
    std::fs::write(format!("{}/nodes.geojson", arguments[2]), serde_json::to_string_pretty(&exported.nodes).unwrap()).unwrap();
    std::fs::write(format!("{}/edges.geojson", arguments[2]), serde_json::to_string_pretty(&exported.edges).unwrap()).unwrap();
}
