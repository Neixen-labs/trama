// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! A CSV of points, read as annotations for the nodes of the network being compiled.
//!
//! KICKOFF's third v0 input. It is not a network on its own: a container needs edges, and a row
//! here carries what some other system knows about a place — a meter reference, an elevation, a
//! customer count — to a node the network already has. The compiler joins them by location, so a
//! row's coordinates must be a node's, and `trama_format::compile` says so when they are not.
//!
//! No CSV dependency: quoting is the only part of the format with a rule, and it is thirty lines.

use serde_json::{Map, Value, json};

/// Column names this recognises as coordinates, in the order it tries them.
const EASTINGS: [&str; 3] = ["longitude", "lon", "x"];
const NORTHINGS: [&str; 3] = ["latitude", "lat", "y"];

/// Point features for `trama_format::compile`, one per row, in WGS 84.
pub fn read(text: &str) -> Result<Vec<Value>, String> {
    let mut rows = rows(text).into_iter();
    let header: Vec<String> =
        rows.next().ok_or("the CSV is empty")?.iter().map(|cell| cell.trim().to_string()).collect();
    let find =
        |candidates: &[&str]| header.iter().position(|name| candidates.contains(&name.to_ascii_lowercase().trim()));
    let (x, y) = match (find(&EASTINGS), find(&NORTHINGS)) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            return Err(format!(
                "the CSV needs a longitude column ({}) and a latitude column ({}); it has: {}",
                EASTINGS.join(", "),
                NORTHINGS.join(", "),
                header.join(", ")
            ));
        }
    };

    let mut features = Vec::new();
    for (line, cells) in rows.enumerate() {
        // A trailing newline is not a row, and neither is a blank line between records.
        if cells.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        if cells.len() != header.len() {
            return Err(format!("row {} has {} cells against the header's {}", line + 2, cells.len(), header.len()));
        }
        let coordinate = |at: usize, what: &str| {
            cells[at]
                .trim()
                .parse::<f64>()
                .map_err(|_| format!("row {}: '{}' is not a {what}", line + 2, cells[at].trim()))
        };
        let mut properties = Map::new();
        for (at, name) in header.iter().enumerate() {
            if at == x || at == y {
                continue;
            }
            // An empty cell is an absent value, not an empty string: SPEC 5 keeps the two apart
            // and a spreadsheet full of blanks would otherwise arrive as a column of "".
            if let Some(value) = typed(cells[at].trim()) {
                properties.insert(name.clone(), value);
            }
        }
        features.push(json!({
            "type": "Feature",
            "properties": Value::Object(properties),
            "geometry": {"type": "Point", "coordinates": [coordinate(x, "longitude")?, coordinate(y, "latitude")?]},
        }));
    }
    Ok(features)
}

/// The narrowest type a cell's text supports, or nothing when the cell is empty.
fn typed(cell: &str) -> Option<Value> {
    if cell.is_empty() {
        return None;
    }
    if let Ok(whole) = cell.parse::<i64>() {
        return Some(json!(whole));
    }
    if let Ok(number) = cell.parse::<f64>() {
        // NaN and infinity have no representation in the format, so they stay text rather than
        // becoming a value SPEC 5 forbids a writer to store.
        if number.is_finite() {
            return Some(json!(number));
        }
    }
    match cell.to_ascii_lowercase().as_str() {
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => Some(Value::String(cell.to_string())),
    }
}

/// RFC 4180 rows: commas separate, quotes protect, and a doubled quote inside quotes is one.
fn rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                cell.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => cells.push(std::mem::take(&mut cell)),
            '\r' if !quoted => {}
            '\n' if !quoted => {
                cells.push(std::mem::take(&mut cell));
                rows.push(std::mem::take(&mut cells));
            }
            other => cell.push(other),
        }
    }
    if !cell.is_empty() || !cells.is_empty() {
        cells.push(cell);
        rows.push(cells);
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_protect_commas_and_double_up_to_escape_themselves() {
        let parsed = rows("a,b\n\"one, two\",\"say \"\"hi\"\"\"\n");
        assert_eq!(parsed, vec![vec!["a", "b"], vec!["one, two", "say \"hi\""]]);
    }

    #[test]
    fn a_row_carries_its_columns_typed_and_keeps_absence_absent() {
        let features = read("lon,lat,meter,elevation,active,note\n-3.7,40.4,7,12.5,true,\n").unwrap();
        assert_eq!(features.len(), 1);
        assert_eq!(features[0]["geometry"]["coordinates"], json!([-3.7, 40.4]));
        assert_eq!(features[0]["properties"]["meter"], json!(7));
        assert_eq!(features[0]["properties"]["elevation"], json!(12.5));
        assert_eq!(features[0]["properties"]["active"], json!(true));
        assert!(features[0]["properties"].get("note").is_none(), "an empty cell is not an empty string");
    }

    #[test]
    fn a_csv_without_coordinates_says_what_it_looked_for_and_what_it_found() {
        let error = read("name,value\nx,1\n").unwrap_err();
        assert!(error.contains("longitude"), "{error}");
        assert!(error.contains("name, value"), "{error}");
    }
}
