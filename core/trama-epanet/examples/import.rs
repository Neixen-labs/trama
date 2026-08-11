use std::collections::BTreeMap;
use std::path::Path;
use trama_format::Importer;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let options: BTreeMap<String, String> = [("source-crs".to_string(), arguments[3].clone())].into_iter().collect();
    let imported = trama_epanet::importer::EpanetImporter
        .load(Path::new(&arguments[1]), &options)
        .unwrap_or_else(|error| panic!("{error}"));
    let bytes = trama_format::compile(&imported.features, &imported.channels, &imported.extras).unwrap();
    std::fs::write(&arguments[2], bytes).unwrap();
}
