// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The command line, exercised as a process rather than as a function call.

use std::path::PathBuf;
use std::process::{Command, Output};

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("trama-cli-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn trama(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trama")).args(arguments).output().unwrap()
}

#[test]
fn compiles_geojson_into_the_container_the_fixture_holds() {
    let out = workspace("geojson").join("network.trama");

    let result =
        trama(&["compile", repository().join("fixtures/network.geojson").to_str().unwrap(), out.to_str().unwrap()]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(repository().join("fixtures/network.trama")).unwrap());
}

#[test]
fn compiles_an_inp_through_the_importer_that_claims_it() {
    let out = workspace("inp").join("net3.trama");

    let result = trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net3.inp").to_str().unwrap(),
        out.to_str().unwrap(),
        "-o",
        "source-crs=EPSG:3857",
    ]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert_eq!(std::fs::read(&out).unwrap(), std::fs::read(repository().join("fixtures/net3.trama")).unwrap());
}

#[test]
fn an_inp_without_its_crs_fails_with_a_message_naming_what_is_missing() {
    let result = trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net1.inp").to_str().unwrap(),
        workspace("nocrs").join("out.trama").to_str().unwrap(),
    ]);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("coordinate reference system"));
}

#[test]
fn an_unclaimed_suffix_says_what_is_missing() {
    let source = workspace("unclaimed").join("network.dwg");
    std::fs::write(&source, "not a network").unwrap();

    let result = trama(&["compile", source.to_str().unwrap(), "/dev/null"]);

    let message = String::from_utf8_lossy(&result.stderr).to_string();
    assert!(message.contains(".dwg") && message.contains("importer"), "{message}");
}

#[test]
fn validate_accepts_a_container_and_rejects_a_corrupted_one() {
    let directory = workspace("validate");
    let good = repository().join("fixtures/network.trama");
    assert!(trama(&["validate", good.to_str().unwrap()]).status.success());

    let mut bytes = std::fs::read(&good).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    let bad = directory.join("bad.trama");
    std::fs::write(&bad, bytes).unwrap();

    assert!(!trama(&["validate", bad.to_str().unwrap()]).status.success());
}

#[test]
fn exports_geojson_and_an_inp_the_way_it_was_imported() {
    let directory = workspace("export");
    let container = repository().join("fixtures/net3.trama");

    assert!(
        trama(&["export", container.to_str().unwrap(), directory.to_str().unwrap(), "--to", "geojson"])
            .status
            .success()
    );
    assert!(directory.join("nodes.geojson").exists() && directory.join("edges.geojson").exists());

    let rebuilt = directory.join("net3.inp");
    let result = trama(&[
        "export",
        container.to_str().unwrap(),
        rebuilt.to_str().unwrap(),
        "--to",
        "inp",
        "-o",
        "crs=EPSG:3857",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    assert!(std::fs::read_to_string(&rebuilt).unwrap().contains("[JUNCTIONS]"));
}

#[test]
fn an_inp_export_without_a_crs_says_so_rather_than_guessing() {
    let result = trama(&[
        "export",
        repository().join("fixtures/net3.trama").to_str().unwrap(),
        workspace("nocrs-export").join("out.inp").to_str().unwrap(),
        "--to",
        "inp",
    ]);

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("coordinate reference system"));
}

#[test]
fn an_importer_declares_the_channels_its_format_implies() {
    let out = workspace("channels").join("net3.trama");
    trama(&[
        "compile",
        repository().join("core/trama-epanet/tests/networks/Net3.inp").to_str().unwrap(),
        out.to_str().unwrap(),
        "-o",
        "source-crs=EPSG:3857",
    ]);

    let container = std::fs::read(&out).unwrap();
    let names: Vec<String> = trama_format_channels(&container);
    // Two from the format, three from topology: what a network reaches and what a cut takes out
    // are questions about any graph, and a solver may only write where the file says it may.
    assert_eq!(names, ["pressure", "flow", "reach", "isolated", "critical"]);
}

fn trama_format_channels(container: &[u8]) -> Vec<String> {
    let sections = trama_format::read_sections(container).unwrap();
    let payload = &sections.iter().find(|s| &s.kind == b"STCH").unwrap().payload;
    let at = |offset: usize| u32::from_le_bytes(payload[offset..offset + 4].try_into().unwrap()) as usize;
    let count = at(0);
    let strings_offset = at(4);
    let mut strings = Vec::new();
    let mut cursor = strings_offset + 4;
    for _ in 0..at(strings_offset) {
        let length = at(cursor);
        strings.push(String::from_utf8_lossy(&payload[cursor + 4..cursor + 4 + length]).into_owned());
        cursor += 4 + length;
    }
    // Each channel record names its own string, and the dictionary deduplicates: three channels
    // sharing the unit "1" contribute one entry between them, so a two-per-channel assumption
    // reads past the end. Records start after the dictionary; the name id is at byte 4 of 24.
    let records_offset = at(8);
    (0..count)
        .map(|index| {
            let record = records_offset + index * 24;
            strings[at(record + 4)].clone()
        })
        .collect()
}

#[test]
fn an_importer_asked_for_by_name_wins_over_the_suffix() {
    // An OpenStreetMap extract is `.json`, which the compiler already claims. Without the flag
    // it compiles as plain GeoJSON and every one-way street quietly becomes two-way.
    let directory = workspace("roads");
    let source = directory.join("extract.json");
    std::fs::write(
        &source,
        r#"{"elements":[{"type":"way","id":1,"nodes":[10,11],
           "geometry":[{"lat":40.416,"lon":-3.704},{"lat":40.417,"lon":-3.703}],
           "tags":{"highway":"residential","oneway":"yes"}}]}"#,
    )
    .unwrap();
    let out = directory.join("roads.trama");

    let result = trama(&["compile", source.to_str().unwrap(), out.to_str().unwrap(), "--importer", "roads"]);

    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    let graph = trama_format::parse_graph(
        &trama_format::read_sections(&std::fs::read(&out).unwrap())
            .unwrap()
            .iter()
            .find(|section| &section.kind == b"GRPH")
            .unwrap()
            .payload,
    )
    .unwrap();
    assert!(graph.edges.iter().all(|edge| edge.directed), "the road importer read the one-way tag");
}

#[test]
fn the_same_file_without_the_flag_takes_the_plain_geojson_path() {
    let directory = workspace("roads-unflagged");
    let source = directory.join("extract.json");
    std::fs::write(&source, r#"{"type":"FeatureCollection","features":[]}"#).unwrap();

    let result = trama(&["compile", source.to_str().unwrap(), directory.join("o.trama").to_str().unwrap()]);

    // It fails as GeoJSON rather than being handed to a road importer, which is the point:
    // nothing changed for a file that was compiling before.
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("out geom"));
}

#[test]
fn an_unknown_importer_name_lists_the_installed_ones() {
    let directory = workspace("roads-unknown");
    let source = directory.join("extract.json");
    std::fs::write(&source, "{}").unwrap();

    let result =
        trama(&["compile", source.to_str().unwrap(), directory.join("o.trama").to_str().unwrap(), "--importer", "osm"]);

    let message = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(message.contains("no installed importer is named 'osm'"), "{message}");
    assert!(message.contains("roads"), "the message names what is available: {message}");
}

#[test]
fn a_csv_of_points_annotates_the_nodes_it_lands_on() {
    let directory = workspace("points");
    let csv = directory.join("meters.csv");
    // Two of the network's own node coordinates, with the columns some other system knows.
    std::fs::write(
        &csv,
        "lon,lat,meter,customers,billed\n-3.67,40.416,\"M-1, north\",42,true\n-3.668,40.417,M-2,7,false\n",
    )
    .unwrap();
    let out = directory.join("annotated.trama");

    let result = trama(&[
        "compile",
        repository().join("fixtures/network.geojson").to_str().unwrap(),
        out.to_str().unwrap(),
        "--points",
        csv.to_str().unwrap(),
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));

    let exported = trama_format::export(&std::fs::read(&out).unwrap()).unwrap();
    let nodes = exported.nodes["features"].as_array().unwrap();
    let annotated: Vec<&serde_json::Value> =
        nodes.iter().filter(|node| node["properties"].get("meter").is_some()).collect();
    assert_eq!(annotated.len(), 2, "both rows should have found a node");

    let first = annotated
        .iter()
        .find(|node| node["properties"]["meter"] == "M-1, north")
        .expect("the quoted comma must survive as one cell");
    assert_eq!(first["properties"]["customers"], 42);
    assert_eq!(first["properties"]["billed"], true);
}

#[test]
fn a_csv_row_that_lands_on_no_node_names_itself_instead_of_vanishing() {
    let directory = workspace("points-adrift");
    let csv = directory.join("meters.csv");
    std::fs::write(&csv, "lon,lat,meter\n-3.67,40.416,on-a-node\n-3.60,40.30,adrift\n").unwrap();

    let result = trama(&[
        "compile",
        repository().join("fixtures/network.geojson").to_str().unwrap(),
        directory.join("out.trama").to_str().unwrap(),
        "--points",
        csv.to_str().unwrap(),
    ]);

    assert!(!result.status.success(), "a row with nowhere to attach must not compile silently");
    let message = String::from_utf8_lossy(&result.stderr);
    assert!(message.contains("-3.6"), "{message}");
    assert!(message.contains("no node"), "{message}");
}

#[test]
fn exports_one_vector_tile_per_geometry_record() {
    let directory = workspace("mvt");
    let out = directory.join("tiles");

    let result = trama(&[
        "export",
        repository().join("fixtures/network.trama").to_str().unwrap(),
        out.to_str().unwrap(),
        "--to",
        "mvt",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));

    // One file per GEOM record, laid out the way a tile server addresses them.
    let container = std::fs::read(repository().join("fixtures/network.trama")).unwrap();
    let expected: Vec<(u32, u32, u32)> = trama_format::read_sections(&container)
        .unwrap()
        .iter()
        .filter(|section| &section.kind == b"GEOM")
        .map(|section| section.key)
        .collect();
    assert!(!expected.is_empty(), "the fixture must have tiles for this to test anything");

    for (z, x, y) in &expected {
        let tile = out.join(z.to_string()).join(x.to_string()).join(format!("{y}.mvt"));
        let bytes = std::fs::read(&tile).unwrap_or_else(|_| panic!("no tile at {}", tile.display()));
        assert!(!bytes.is_empty(), "{} is empty", tile.display());
        // Layer names are length-delimited strings in the protobuf, so they are literally there:
        // enough to tell a written tile from a plausible-looking pile of bytes.
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("edges"), "{} names no edges layer", tile.display());
        assert!(text.contains("nodes"), "{} names no nodes layer", tile.display());
    }
}
