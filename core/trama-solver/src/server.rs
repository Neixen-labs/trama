// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! The server runtime of `docs/SOLVER_CONTRACT.md` section 6: POST /solve, Server-Sent Events.
//!
//! No web framework. A solver is a plugin, not the product's backend, and one endpoint that
//! streams bytes is a `TcpListener` and a loop.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

use serde_json::{Value, json};

/// One event per 256 deltas: the contract allows batching, and an event per delta would spend
/// more bytes on framing than on payload.
const DELTAS_PER_EVENT: usize = 256;
const MAXIMUM_CONTAINER_BYTES: usize = 64 * 1024 * 1024;

pub struct Request {
    pub container: Vec<u8>,
    pub params: Value,
    pub t0_seconds: f32,
    pub t1_seconds: f32,
}

/// What a solver must tell the runtime about itself.
pub trait Solver: Send + Sync {
    fn id(&self) -> &'static str;
    fn contract_versions(&self) -> &'static [&'static str];
    fn solve(&self, request: &Request) -> Result<Vec<u8>, Rejection>;
}

pub struct Rejection {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl Rejection {
    pub fn input(message: impl Into<String>) -> Self {
        Self { status: 400, code: "invalid_input", message: message.into() }
    }

    pub fn request(message: impl Into<String>) -> Self {
        Self { status: 400, code: "invalid_request", message: message.into() }
    }

    /// 503, not 400: the request was fine and this host cannot serve it.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self { status: 503, code: "solver_unavailable", message: message.into() }
    }
}

pub fn serve(solver: &dyn Solver, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("{} listening on http://127.0.0.1:{}/solve", solver.id(), listener.local_addr()?.port());
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => handle(solver, stream),
            // One dropped connection is not a reason to stop serving the next.
            Err(_error) => continue,
        }
    }
    Ok(())
}

fn handle(solver: &dyn Solver, mut stream: TcpStream) {
    let (method, path, body) = match read_request(&mut stream) {
        Ok(parts) => parts,
        Err(_error) => return,
    };
    if method == "OPTIONS" {
        // A browser preflights POST with a JSON content type, so /solve must answer it.
        let _ = stream.write_all(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nAccess-Control-Max-Age: 600\r\nContent-Length: 0\r\n\r\n",
        );
        return;
    }
    if path != "/solve" {
        reject(
            &mut stream,
            &Rejection { status: 404, code: "invalid_request", message: format!("no such endpoint {path}") },
        );
        return;
    }
    match prepare(solver, &body).and_then(|request| solver.solve(&request)) {
        Ok(deltas) => stream_deltas(&mut stream, solver, &deltas),
        Err(rejection) => reject(&mut stream, &rejection),
    }
}

fn prepare(solver: &dyn Solver, body: &[u8]) -> Result<Request, Rejection> {
    let parsed: Value =
        serde_json::from_slice(body).map_err(|error| Rejection::request(format!("body is not JSON: {error}")))?;
    if !parsed.is_object() {
        return Err(Rejection::request("body must be a JSON object"));
    }
    let version = parsed["contract_version"].as_str().unwrap_or_default();
    if !solver.contract_versions().contains(&version) {
        return Err(Rejection {
            status: 400,
            code: "unsupported_contract",
            message: format!("this solver speaks {}", solver.contract_versions().join(", ")),
        });
    }
    let url = parsed["trama"]["url"]
        .as_str()
        .filter(|url| !url.is_empty())
        .ok_or_else(|| Rejection::request("trama.url is required"))?;
    Ok(Request {
        container: fetch(url)?,
        params: parsed.get("params").cloned().unwrap_or_else(|| json!({})),
        t0_seconds: parsed["t0_seconds"].as_f64().unwrap_or(0.0) as f32,
        t1_seconds: parsed["t1_seconds"].as_f64().unwrap_or(0.0) as f32,
    })
}

/// Contract section 6 requires an absolute HTTPS URL. `http://localhost` is also accepted so
/// this can run against the local demo; a deployed solver MUST NOT, and that is the access
/// policy the contract asks it to document.
fn fetch(url: &str) -> Result<Vec<u8>, Rejection> {
    let rest = url
        .strip_prefix("http://localhost")
        .or_else(|| url.strip_prefix("http://127.0.0.1"))
        .ok_or_else(|| Rejection::request("trama.url must be https, or http on localhost"))?;
    let (authority, path) = match rest.split_once('/') {
        Some((port, path)) => (format!("127.0.0.1{port}"), format!("/{path}")),
        None => (format!("127.0.0.1{rest}"), "/".to_string()),
    };
    let mut stream = TcpStream::connect(&authority).map_err(|error| Rejection {
        status: 400,
        code: "fetch_failed",
        message: error.to_string(),
    })?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {authority}\r\nAccept: */*\r\n\r\n");
    stream.write_all(request.as_bytes()).map_err(|error| Rejection {
        status: 400,
        code: "fetch_failed",
        message: error.to_string(),
    })?;
    let mut response = Vec::new();
    stream.take(MAXIMUM_CONTAINER_BYTES as u64 + 1).read_to_end(&mut response).map_err(|error| Rejection {
        status: 400,
        code: "fetch_failed",
        message: error.to_string(),
    })?;
    let separator = response.windows(4).position(|window| window == b"\r\n\r\n").ok_or_else(|| Rejection {
        status: 400,
        code: "fetch_failed",
        message: "no HTTP response headers".into(),
    })?;
    Ok(response[separator + 4..].to_vec())
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<(String, String, Vec<u8>)> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut start = String::new();
    reader.read_line(&mut start)?;
    let mut parts = start.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body)?;
    }
    Ok((method, path, body))
}

fn stream_deltas(stream: &mut TcpStream, solver: &dyn Solver, deltas: &[u8]) {
    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n",
    );
    let _ = write!(
        stream,
        "event: ready\ndata: {}\n\n",
        json!({"contract_version": solver.contract_versions().last(), "solver_id": solver.id()})
    );
    for batch in deltas.chunks(DELTAS_PER_EVENT * crate::DELTA_BYTES) {
        let _ = write!(stream, "event: delta\ndata: {}\n\n", base64(batch));
    }
    let _ = write!(stream, "event: complete\ndata: {}\n\n", json!({"delta_count": deltas.len() / crate::DELTA_BYTES}));
}

fn reject(stream: &mut TcpStream, rejection: &Rejection) {
    // A failure before the stream starts is still one terminal error event, per section 6.
    let body = format!("event: error\ndata: {}\n\n", json!({"code": rejection.code, "message": rejection.message}));
    let _ = write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: text/event-stream\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        rejection.status,
        if rejection.status == 404 { "Not Found" } else { "Bad Request" },
        body.len(),
        body
    );
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(data.len().div_ceil(3) * 4);
    for group in data.chunks(3) {
        let triple = group
            .iter()
            .enumerate()
            .fold(0u32, |packed, (index, byte)| packed | (u32::from(*byte) << (16 - 8 * index)));
        for slot in 0..4 {
            if slot <= group.len() {
                encoded.push(ALPHABET[(triple >> (18 - 6 * slot) & 0x3F) as usize] as char);
            } else {
                encoded.push('=');
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    /// Hand-rolled encoders are where silent corruption lives, so this pins RFC 4648 vectors.
    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(super::base64(plain.as_bytes()), encoded, "encoding {plain:?}");
        }
    }

    #[test]
    fn a_delta_is_eighteen_bytes_little_endian() {
        let record = crate::pack(0x0102_0304_0506_0708, 7, 1.0, -2.0);

        assert_eq!(record.len(), 18);
        assert_eq!(&record[0..8], &[8, 7, 6, 5, 4, 3, 2, 1]);
        assert_eq!(&record[8..10], &[7, 0]);
        assert_eq!(f32::from_le_bytes(record[14..18].try_into().unwrap()), -2.0);
    }
}
