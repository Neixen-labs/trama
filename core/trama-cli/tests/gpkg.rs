// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! GeoPackage export: the anti-lock-in promise, checked by reading the database back.
//!
//! Nothing here trusts the writer's own view of what it wrote. Every assertion goes through
//! SQLite, which is what a reader on the other side will do.

use std::path::PathBuf;
use std::process::{Command, Output};

use rusqlite::Connection;

fn repository() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

fn workspace(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("trama-gpkg-{name}"));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn trama(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_trama")).args(arguments).output().unwrap()
}

fn export(name: &str) -> PathBuf {
    let out = workspace(name).join("network.gpkg");
    let result = trama(&[
        "export",
        repository().join("fixtures/network.trama").to_str().unwrap(),
        out.to_str().unwrap(),
        "--to",
        "gpkg",
    ]);
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    out
}

#[test]
fn writes_the_tables_a_geopackage_reader_looks_for() {
    let database = Connection::open(export("tables")).unwrap();

    let application_id: i64 = database.query_row("PRAGMA application_id", [], |row| row.get(0)).unwrap();
    assert_eq!(application_id, 0x4750_4b47, "the file must announce itself as GPKG");

    let layers: Vec<(String, String, i64)> = database
        .prepare("SELECT table_name, data_type, srs_id FROM gpkg_contents ORDER BY table_name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        layers,
        vec![("edges".to_string(), "features".to_string(), 3857), ("nodes".to_string(), "features".to_string(), 3857)]
    );

    let geometry: Vec<(String, String)> = database
        .prepare("SELECT table_name, geometry_type_name FROM gpkg_geometry_columns ORDER BY table_name")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        geometry,
        vec![("edges".to_string(), "LINESTRING".to_string()), ("nodes".to_string(), "POINT".to_string())]
    );
}

#[test]
fn every_entity_arrives_with_its_stable_id_and_its_properties() {
    let container = std::fs::read(repository().join("fixtures/network.trama")).unwrap();
    let expected = trama_format::export_projected(&container).unwrap();
    let database = Connection::open(export("entities")).unwrap();

    for (layer, collection) in [("nodes", &expected.nodes), ("edges", &expected.edges)] {
        let features = collection["features"].as_array().unwrap();
        let count: i64 = database.query_row(&format!("SELECT count(*) FROM {layer}"), [], |row| row.get(0)).unwrap();
        assert_eq!(count as usize, features.len(), "{layer} lost or gained entities");

        let ids: Vec<String> = database
            .prepare(&format!("SELECT trama_id FROM {layer} ORDER BY fid"))
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        let source: Vec<String> =
            features.iter().map(|f| f["properties"]["_trama_id"].as_str().unwrap().to_string()).collect();
        assert_eq!(ids, source, "{layer} ids must survive the export, in order");
    }

    // The fixture carries one of each v0 type and, on two of its three edges, the absence of
    // some of them — which is the distinction SPEC 5 insists is not `false`, `0` or `""`.
    for feature in expected.edges["features"].as_array().unwrap() {
        let properties = &feature["properties"];
        let identity = properties["_trama_id"].as_str().unwrap();
        let row: (Option<String>, Option<f64>, Option<i64>, Option<i64>) = database
            .query_row("SELECT label, loss, rank, active FROM edges WHERE trama_id = ?1", [identity], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap();
        assert_eq!(row.0.as_deref(), properties["label"].as_str(), "label of {identity}");
        assert_eq!(row.1, properties["loss"].as_f64(), "loss of {identity}");
        assert_eq!(row.2, properties["rank"].as_i64(), "rank of {identity}");
        assert_eq!(row.3, properties["active"].as_bool().map(i64::from), "active of {identity}");
    }
}

#[test]
fn geometry_is_a_geopackage_blob_holding_the_projected_coordinates() {
    let container = std::fs::read(repository().join("fixtures/network.trama")).unwrap();
    let expected = trama_format::export_projected(&container).unwrap();
    let database = Connection::open(export("geometry")).unwrap();

    let blob: Vec<u8> =
        database.query_row("SELECT geom FROM nodes ORDER BY fid LIMIT 1", [], |row| row.get(0)).unwrap();
    assert_eq!(&blob[0..2], b"GP", "an OGC reader keys off this magic");
    assert_eq!(i32::from_le_bytes(blob[4..8].try_into().unwrap()), 3857);
    assert_eq!(u32::from_le_bytes(blob[9..13].try_into().unwrap()), 1, "WKB type 1 is Point");
    let x = f64::from_le_bytes(blob[13..21].try_into().unwrap());
    let y = f64::from_le_bytes(blob[21..29].try_into().unwrap());
    let point = &expected.nodes["features"][0]["geometry"]["coordinates"];
    assert_eq!((x, y), (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()));
    // Metres, not degrees: a longitude would fit in three digits and this must not.
    assert!(x.abs() > 1000.0, "EPSG:3857 easting expected, got {x}");

    let line: Vec<u8> =
        database.query_row("SELECT geom FROM edges ORDER BY fid LIMIT 1", [], |row| row.get(0)).unwrap();
    assert_eq!(u32::from_le_bytes(line[9..13].try_into().unwrap()), 2, "WKB type 2 is LineString");
    let vertices = u32::from_le_bytes(line[13..17].try_into().unwrap()) as usize;
    assert_eq!(vertices, expected.edges["features"][0]["geometry"]["coordinates"].as_array().unwrap().len());
}

#[test]
fn the_same_container_exports_the_same_bytes() {
    // The compiler's determinism is worth nothing if the way out of the format is stamped with
    // a clock: two people exporting the same file must be able to diff the results.
    assert_eq!(std::fs::read(export("determinism-a")).unwrap(), std::fs::read(export("determinism-b")).unwrap());
}
