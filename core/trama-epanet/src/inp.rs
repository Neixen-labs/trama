// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Reading and writing EPANET `.inp` text.
//!
//! This module knows the file's shape — bracketed sections of whitespace-separated fields with
//! `;` comments — and nothing about hydraulics. Section order and raw lines are preserved
//! because most of a `.inp` travels back out untouched.

/// Sections in file order. A name may repeat: Net3 declares `[REACTIONS]` twice.
pub struct Document {
    pub sections: Vec<(String, Vec<String>)>,
}

impl Document {
    pub fn lines(&self, name: &str) -> impl Iterator<Item = &String> {
        self.sections.iter().filter(move |(section, _)| section == name).flat_map(|(_, body)| body)
    }

    /// Field rows of a section, with comments and blank lines dropped.
    pub fn rows(&self, name: &str) -> Vec<Vec<String>> {
        self.lines(name).map(|line| values(line)).filter(|fields| !fields.is_empty()).collect()
    }

    pub fn without(&self, names: &[&str]) -> Document {
        Document {
            sections: self.sections.iter().filter(|(name, _body)| !names.contains(&name.as_str())).cloned().collect(),
        }
    }
}

pub fn parse(text: &str) -> Document {
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut name = String::new();
    let mut body: Vec<String> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.starts_with('[') && stripped.ends_with(']') {
            sections.push((name, body));
            name = stripped[1..stripped.len() - 1].trim().to_uppercase();
            body = Vec::new();
        } else {
            body.push(line.to_string());
        }
    }
    sections.push((name, body));
    // The leading group holds whatever preceded the first header, usually nothing.
    Document {
        sections: sections
            .into_iter()
            .filter(|(name, body)| !name.is_empty() || body.iter().any(|line| !line.trim().is_empty()))
            .collect(),
    }
}

pub fn serialize(document: &Document) -> String {
    let mut text = String::new();
    for (name, body) in &document.sections {
        if !name.is_empty() {
            text.push_str(&format!("[{name}]\n"));
        }
        for line in body {
            text.push_str(line);
            text.push('\n');
        }
    }
    text
}

/// The fields of one line: everything before `;`, split on whitespace.
pub fn values(line: &str) -> Vec<String> {
    line.split(';').next().unwrap_or("").split_whitespace().map(str::to_string).collect()
}

/// A section built from field rows, laid out the way EPANET's own writer does.
pub fn section(name: &str, header: &str, rows: Vec<Vec<String>>) -> (String, Vec<String>) {
    let mut body = vec![format!(";{header}")];
    body.extend(rows.into_iter().map(|fields| format!(" {}", fields.join("\t"))));
    body.push(String::new());
    (name.to_string(), body)
}

/// Render a value back into a field, without the trailing `.0` EPANET never writes.
pub fn text(value: f64) -> String {
    if value == value.trunc() && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{value}") }
}
