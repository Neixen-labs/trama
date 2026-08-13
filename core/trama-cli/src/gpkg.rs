// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! GeoPackage export, per SPEC 9.
//!
//! A `.gpkg` is a SQLite database with the tables OGC requires and geometry stored as blobs, so
//! this is the one place in the workspace that links SQLite. It lives in the command line rather
//! than in `trama-format` on purpose: the browser module compiles the format crate to
//! WebAssembly, and it has no business carrying a C database engine to do it.

use std::collections::BTreeMap;
use std::path::Path;

use rusqlite::{Connection, params};
use serde_json::Value;
use trama_format::Export;

/// EPSG:3857, the projection SPEC 1.1 stores and SPEC 9 writes out.
const MERCATOR: i32 = 3857;
const MERCATOR_WKT: &str = "PROJCS[\"WGS 84 / Pseudo-Mercator\",GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563,AUTHORITY[\"EPSG\",\"7030\"]],AUTHORITY[\"EPSG\",\"6326\"]],PRIMEM[\"Greenwich\",0,AUTHORITY[\"EPSG\",\"8901\"]],UNIT[\"degree\",0.0174532925199433,AUTHORITY[\"EPSG\",\"9122\"]],AUTHORITY[\"EPSG\",\"4326\"]],PROJECTION[\"Mercator_1SP\"],PARAMETER[\"central_meridian\",0],PARAMETER[\"scale_factor\",1],PARAMETER[\"false_easting\",0],PARAMETER[\"false_northing\",0],UNIT[\"metre\",1,AUTHORITY[\"EPSG\",\"9001\"]],AXIS[\"Easting\",EAST],AXIS[\"Northing\",NORTH],AUTHORITY[\"EPSG\",\"3857\"]]";
const WGS84_WKT: &str = "GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563,AUTHORITY[\"EPSG\",\"7030\"]],AUTHORITY[\"EPSG\",\"6326\"]],PRIMEM[\"Greenwich\",0,AUTHORITY[\"EPSG\",\"8901\"]],UNIT[\"degree\",0.0174532925199433,AUTHORITY[\"EPSG\",\"9122\"]],AUTHORITY[\"EPSG\",\"4326\"]]";
// The container carries no clock, and the same input must give the same bytes: SPEC's golden
// test is the compiler's, but an exporter that stamped the wall clock would break the property
// for anyone diffing two exports. GeoPackage requires the column, not that it be truthful.
const EPOCH: &str = "1970-01-01T00:00:00.000Z";

/// Writes `export` as a GeoPackage at `path`, replacing whatever was there.
pub fn write(export: &Export, path: &Path) -> Result<(), String> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|error| error.to_string())?;
    }
    let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
    schema(&connection)?;
    let transaction = connection.transaction().map_err(|error| error.to_string())?;
    layer(&transaction, "nodes", "POINT", features(&export.nodes)?)?;
    layer(&transaction, "edges", "LINESTRING", features(&export.edges)?)?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(())
}

fn features(collection: &Value) -> Result<&Vec<Value>, String> {
    collection["features"].as_array().ok_or_else(|| "export is not a FeatureCollection".into())
}

fn schema(connection: &Connection) -> Result<(), String> {
    // 0x47504B47 is "GPKG": the application id a reader checks before trusting the tables.
    connection
        .execute_batch(
            "PRAGMA application_id = 1196444487;
             PRAGMA user_version = 10300;
             CREATE TABLE gpkg_spatial_ref_sys (
               srs_name TEXT NOT NULL, srs_id INTEGER NOT NULL PRIMARY KEY,
               organization TEXT NOT NULL, organization_coordsys_id INTEGER NOT NULL,
               definition TEXT NOT NULL, description TEXT);
             CREATE TABLE gpkg_contents (
               table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL,
               identifier TEXT UNIQUE, description TEXT DEFAULT '',
               last_change DATETIME NOT NULL, min_x DOUBLE, min_y DOUBLE, max_x DOUBLE,
               max_y DOUBLE, srs_id INTEGER,
               CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id));
             CREATE TABLE gpkg_geometry_columns (
               table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL,
               srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL,
               CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name),
               CONSTRAINT fk_gc_srs FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id));",
        )
        .map_err(|error| error.to_string())?;
    // The two undefined systems are required rows whatever the file holds, per GeoPackage 1.3.
    for (id, name, organization, code, definition) in [
        (-1, "Undefined cartesian SRS", "NONE", -1, "undefined"),
        (0, "Undefined geographic SRS", "NONE", 0, "undefined"),
        (4326, "WGS 84 geodetic", "EPSG", 4326, WGS84_WKT),
        (MERCATOR, "WGS 84 / Pseudo-Mercator", "EPSG", MERCATOR, MERCATOR_WKT),
    ] {
        connection
            .execute(
                "INSERT INTO gpkg_spatial_ref_sys (srs_id, srs_name, organization, organization_coordsys_id, definition)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, name, organization, code, definition],
            )
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// One feature table, its `gpkg_contents` row and its `gpkg_geometry_columns` row.
fn layer(connection: &Connection, name: &str, kind: &str, features: &[Value]) -> Result<(), String> {
    let columns = columns(features);
    let declarations: String =
        columns.iter().map(|(key, sql)| format!(", {} {sql}", quoted(key))).collect::<Vec<String>>().join("");
    connection
        .execute_batch(&format!(
            "CREATE TABLE {name} (fid INTEGER PRIMARY KEY, geom BLOB, trama_id TEXT NOT NULL{declarations});"
        ))
        .map_err(|error| error.to_string())?;

    let names: String =
        columns.iter().map(|(key, _sql)| format!(", {}", quoted(key))).collect::<Vec<String>>().join("");
    let placeholders: String = (0..columns.len()).map(|at| format!(", ?{}", at + 4)).collect::<Vec<String>>().join("");
    let mut insert = connection
        .prepare(&format!("INSERT INTO {name} (fid, geom, trama_id{names}) VALUES (?1, ?2, ?3{placeholders})"))
        .map_err(|error| error.to_string())?;

    let mut bounds: Option<(f64, f64, f64, f64)> = None;
    for (index, feature) in features.iter().enumerate() {
        let points = coordinates(&feature["geometry"])?;
        for (x, y) in &points {
            bounds = Some(match bounds {
                None => (*x, *y, *x, *y),
                Some((min_x, min_y, max_x, max_y)) => (min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y)),
            });
        }
        let identity = feature["properties"]["_trama_id"].as_str().ok_or("feature has no _trama_id")?;
        let mut values: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(index as i64 + 1), Box::new(geometry_blob(&points, kind)), Box::new(identity.to_string())];
        for (key, _sql) in &columns {
            values.push(match &feature["properties"][key] {
                Value::Number(number) if number.is_i64() => Box::new(number.as_i64()),
                Value::Number(number) => Box::new(number.as_f64()),
                Value::String(text) => Box::new(text.clone()),
                Value::Bool(flag) => Box::new(i64::from(*flag)),
                _ => Box::new(None::<i64>),
            });
        }
        insert
            .execute(rusqlite::params_from_iter(values.iter().map(|value| value.as_ref())))
            .map_err(|error| error.to_string())?;
    }

    let (min_x, min_y, max_x, max_y) = bounds.unwrap_or((0.0, 0.0, 0.0, 0.0));
    connection
        .execute(
            "INSERT INTO gpkg_contents (table_name, data_type, identifier, description, last_change,
                                        min_x, min_y, max_x, max_y, srs_id)
             VALUES (?1, 'features', ?1, '', ?2, ?3, ?4, ?5, ?6, ?7)",
            params![name, EPOCH, min_x, min_y, max_x, max_y, MERCATOR],
        )
        .map_err(|error| error.to_string())?;
    connection
        .execute(
            "INSERT INTO gpkg_geometry_columns (table_name, column_name, geometry_type_name, srs_id, z, m)
             VALUES (?1, 'geom', ?2, ?3, 0, 0)",
            params![name, kind, MERCATOR],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

/// Every property key in the layer, with the SQLite type its first present value implies.
///
/// A column's type comes from the data because the format's own typing does not survive the
/// export: SPEC 9 says as much about GeoJSON, and SQLite is the looser of the two anyway.
fn columns(features: &[Value]) -> Vec<(String, &'static str)> {
    let mut found: BTreeMap<String, &'static str> = BTreeMap::new();
    for feature in features {
        let Some(properties) = feature["properties"].as_object() else { continue };
        for (key, value) in properties {
            if key == "_trama_id" {
                continue;
            }
            let declared = match value {
                Value::Number(number) if number.is_i64() => "INTEGER",
                Value::Number(_) => "REAL",
                Value::Bool(_) => "INTEGER",
                Value::String(_) => "TEXT",
                _ => continue,
            };
            found.entry(key.clone()).or_insert(declared);
        }
    }
    found.into_iter().collect()
}

/// A SQLite identifier: property keys carry colons and spaces from whoever wrote the source.
fn quoted(key: &str) -> String {
    format!("\"{}\"", key.replace('"', "\"\""))
}

fn coordinates(geometry: &Value) -> Result<Vec<(f64, f64)>, String> {
    let pair = |value: &Value| -> Result<(f64, f64), String> {
        let point = value.as_array().ok_or("geometry coordinate is not a pair")?;
        Ok((
            point.first().and_then(Value::as_f64).ok_or("geometry coordinate has no x")?,
            point.get(1).and_then(Value::as_f64).ok_or("geometry coordinate has no y")?,
        ))
    };
    match geometry["type"].as_str() {
        Some("Point") => Ok(vec![pair(&geometry["coordinates"])?]),
        Some("LineString") => {
            geometry["coordinates"].as_array().ok_or("LineString has no coordinates")?.iter().map(pair).collect()
        }
        other => Err(format!("v0 exports Point and LineString, not {}", other.unwrap_or("an untyped geometry"))),
    }
}

/// GeoPackageBinary: the "GP" header OGC wraps around a plain WKB geometry.
///
/// Flags are `0x01`: little-endian, no envelope. An envelope is optional and every reader
/// computes it when it is absent, so writing one would be a second copy of the bounds.
fn geometry_blob(points: &[(f64, f64)], kind: &str) -> Vec<u8> {
    let mut blob = vec![b'G', b'P', 0, 0x01];
    blob.extend_from_slice(&MERCATOR.to_le_bytes());
    blob.push(1); // WKB byte order: little-endian
    if kind == "POINT" {
        blob.extend_from_slice(&1u32.to_le_bytes());
    } else {
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(&(points.len() as u32).to_le_bytes());
    }
    for (x, y) in points {
        blob.extend_from_slice(&x.to_le_bytes());
        blob.extend_from_slice(&y.to_le_bytes());
    }
    blob
}
