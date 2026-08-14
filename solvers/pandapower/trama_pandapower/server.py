# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""``POST /solve`` over Server-Sent Events, per ``docs/SOLVER_CONTRACT.md`` section 6.

No web framework, for the reason the Rust runtime gives: a solver is a plugin, not the
product's backend, and one endpoint that streams bytes is a socket and a loop. It also means
running this needs pandapower and nothing else.

**Access policy**, which section 6 requires an implementation to document: `trama.url` must be
an absolute HTTPS URL, fetched with no credentials and no redirects followed off the origin it
started on, and capped at 64 MB. Passing ``--allow-http`` additionally permits `http://` on
loopback addresses, which is for a developer serving a container from their own machine and is
refused for anything else. There is no allowlist beyond that: deploy this behind one.
"""

from __future__ import annotations

import base64
import hashlib
import ipaddress
import json
import socket
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from .container import Container, MalformedContainer
from .solver import Unsolvable, solve

SOLVER_ID = "pandapower"
CONTRACT_VERSIONS = ("0.1.0", "0.2.0", "0.3.0")
#: One event per 256 deltas, matching the Rust runtime: an event per delta is mostly framing.
DELTAS_PER_EVENT = 256
DELTA_BYTES = 18
MAXIMUM_CONTAINER_BYTES = 64 * 1024 * 1024


class Refused(Exception):
    """A request this server will not serve, carrying the contract's own error code."""

    def __init__(self, code: str, message: str, status: int = 400) -> None:
        super().__init__(message)
        self.code = code
        self.message = message
        self.status = status


def fetch(url: str, expected_sha256: str | None, allow_http: bool) -> bytes:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme == "http" and allow_http and _is_loopback(parsed.hostname):
        pass
    elif parsed.scheme != "https":
        raise Refused("invalid_request", "trama.url must be an absolute HTTPS URL")
    try:
        with urllib.request.urlopen(url, timeout=30) as response:  # noqa: S310 - scheme checked above
            data = response.read(MAXIMUM_CONTAINER_BYTES + 1)
    except Exception as failure:
        raise Refused("fetch_failed", f"could not fetch the container: {failure}") from failure
    if len(data) > MAXIMUM_CONTAINER_BYTES:
        raise Refused("fetch_failed", f"the container exceeds {MAXIMUM_CONTAINER_BYTES} bytes")
    if expected_sha256 is not None:
        digest = hashlib.sha256(data).hexdigest()
        if digest != expected_sha256.lower():
            raise Refused("fetch_failed", f"the container hashes to {digest}, not the sha256 given")
    return data


def _is_loopback(hostname: str | None) -> bool:
    if hostname is None:
        return False
    if hostname == "localhost":
        return True
    try:
        return ipaddress.ip_address(hostname).is_loopback
    except ValueError:
        return False


def run(body: dict[str, Any], allow_http: bool) -> bytes:
    """Validate one request and return its deltas, or raise `Refused`."""
    version = body.get("contract_version")
    if version not in CONTRACT_VERSIONS:
        raise Refused("unsupported_contract", f"this solver speaks {', '.join(CONTRACT_VERSIONS)}, not {version!r}")
    url = (body.get("trama") or {}).get("url")
    if not isinstance(url, str):
        raise Refused("invalid_request", "a request names its container in trama.url")
    t0 = float(body.get("t0_seconds", 0.0))
    t1 = float(body.get("t1_seconds", t0))
    params = body.get("params") or {}
    if not isinstance(params, dict):
        raise Refused("invalid_request", "params is an object")

    data = fetch(url, (body.get("trama") or {}).get("sha256"), allow_http)
    try:
        return solve(Container(data), t0, t1, params)
    except MalformedContainer as failure:
        raise Refused("invalid_input", str(failure)) from failure
    except Unsolvable as failure:
        raise Refused("invalid_input", str(failure)) from failure
    except Exception as failure:
        raise Refused("execution_failed", str(failure), status=500) from failure


class Handler(BaseHTTPRequestHandler):
    allow_http = False
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args: Any) -> None:
        """Quiet by default: a solver's stdout is not a request log."""

    def do_OPTIONS(self) -> None:  # noqa: N802 - the name http.server dispatches on
        self.send_response(204)
        self._cors()
        self.send_header("Access-Control-Allow-Methods", "POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/solve":
            self._reject(Refused("invalid_request", f"no such endpoint {self.path}", status=404))
            return
        try:
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length", 0))) or b"{}")
            if not isinstance(body, dict):
                raise Refused("invalid_request", "the request body is a JSON object")
            deltas = run(body, self.allow_http)
        except json.JSONDecodeError as failure:
            self._reject(Refused("invalid_request", f"the request body is not JSON: {failure}"))
            return
        except Refused as refusal:
            self._reject(refusal)
            return
        self._stream(deltas)

    def _cors(self) -> None:
        self.send_header("Access-Control-Allow-Origin", "*")

    def _reject(self, refusal: Refused) -> None:
        """Before the stream opens a refusal is an HTTP status; the contract's error event is
        for a failure that happens once the stream is already text/event-stream."""
        payload = json.dumps({"code": refusal.code, "message": refusal.message}).encode("utf-8")
        self.send_response(refusal.status)
        self._cors()
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def _stream(self, deltas: bytes) -> None:
        self.send_response(200)
        self._cors()
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self._event("ready", json.dumps({"contract_version": CONTRACT_VERSIONS[-1], "solver_id": SOLVER_ID}))
        batch = DELTAS_PER_EVENT * DELTA_BYTES
        for start in range(0, len(deltas), batch):
            self._event("delta", base64.b64encode(deltas[start : start + batch]).decode("ascii"))
        self._event("complete", json.dumps({"delta_count": len(deltas) // DELTA_BYTES}))

    def _event(self, name: str, data: str) -> None:
        self.wfile.write(f"event: {name}\ndata: {data}\n\n".encode("utf-8"))


def serve(port: int = 8080, allow_http: bool = False) -> None:
    Handler.allow_http = allow_http
    server = ThreadingHTTPServer(("127.0.0.1", port), Handler)
    server.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    print(f"{SOLVER_ID} listening on http://127.0.0.1:{server.server_address[1]}/solve")
    server.serve_forever()


def main() -> None:
    import argparse

    parser = argparse.ArgumentParser(description="The TRAMA pandapower solver.")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument(
        "--allow-http",
        action="store_true",
        help="also accept http:// container URLs on loopback, for local development",
    )
    arguments = parser.parse_args()
    serve(arguments.port, arguments.allow_http)


if __name__ == "__main__":
    main()
