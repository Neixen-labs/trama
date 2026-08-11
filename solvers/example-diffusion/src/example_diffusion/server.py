# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The server runtime of `docs/SOLVER_CONTRACT.md` section 6: POST /solve, Server-Sent Events.

Standard library only. A solver is a plugin, not the product's backend, and http.server is
enough for one endpoint that streams bytes.
"""

from __future__ import annotations

import base64
import json
import urllib.error
import urllib.request
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from example_diffusion.solver import DELTA, InvalidInput, Parameters, solve

SOLVER_ID = "example-diffusion"
CONTRACT_VERSION = "0.1.0"
# One event per 256 deltas: contract section 6 allows batching, and an event per delta would
# spend more bytes on SSE framing than on payload.
DELTAS_PER_EVENT = 256
MAXIMUM_CONTAINER_BYTES = 64 * 1024 * 1024


class SolveHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self) -> None:  # the name BaseHTTPRequestHandler dispatches to
        if self.path != "/solve":
            self._reject(404, "invalid_request", f"no such endpoint {self.path}")
            return
        try:
            request = self._read_request()
            container = fetch_container(request["trama"]["url"])
            deltas = solve(
                container,
                _parameters(request.get("params") or {}),
                float(request.get("t0_seconds", 0)),
                float(request.get("t1_seconds", 0)),
            )
        except _Rejected as rejected:
            self._reject(400, rejected.code, rejected.message)
            return
        except InvalidInput as error:
            self._reject(400, "invalid_input", str(error))
            return
        except (OSError, urllib.error.URLError) as error:
            self._reject(400, "fetch_failed", str(error))
            return

        self._begin_stream()
        self._event("ready", {"contract_version": CONTRACT_VERSION, "solver_id": SOLVER_ID})
        count = len(deltas) // DELTA.size
        batch = DELTAS_PER_EVENT * DELTA.size
        for start in range(0, len(deltas), batch):
            self._raw_event("delta", base64.b64encode(deltas[start : start + batch]).decode())
        self._event("complete", {"delta_count": count})
        self.wfile.write(b"0\r\n\r\n")

    def do_OPTIONS(self) -> None:  # the name BaseHTTPRequestHandler dispatches to
        """A browser preflights POST with a JSON content type, so /solve must answer it."""
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.send_header("Access-Control-Max-Age", "600")
        self.send_header("Content-Length", "0")
        self.end_headers()

    def log_message(self, format: str, *args: object) -> None:
        """Silence the default stderr access log; tests and demos do not want it."""

    def _read_request(self) -> dict:
        length = int(self.headers.get("Content-Length") or 0)
        try:
            request = json.loads(self.rfile.read(length) or b"{}")
        except json.JSONDecodeError as error:
            raise _Rejected("invalid_request", f"body is not JSON: {error}") from error
        if not isinstance(request, dict):
            raise _Rejected("invalid_request", "body must be a JSON object")
        if request.get("contract_version") != CONTRACT_VERSION:
            raise _Rejected("unsupported_contract", f"this solver speaks {CONTRACT_VERSION}")
        url = (request.get("trama") or {}).get("url")
        if not isinstance(url, str) or not url:
            raise _Rejected("invalid_request", "trama.url is required")
        return request

    def _begin_stream(self) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Transfer-Encoding", "chunked")
        self.end_headers()

    def _event(self, name: str, data: dict) -> None:
        self._raw_event(name, json.dumps(data, separators=(",", ":")))

    def _raw_event(self, name: str, data: str) -> None:
        chunk = f"event: {name}\ndata: {data}\n\n".encode()
        self.wfile.write(f"{len(chunk):X}\r\n".encode() + chunk + b"\r\n")

    def _reject(self, status: int, code: str, message: str) -> None:
        """A failure before the stream starts is still one terminal error event, per section 6."""
        self.send_response(status)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Access-Control-Allow-Origin", "*")
        body = f'event: error\ndata: {json.dumps({"code": code, "message": message}, separators=(",", ":"))}\n\n'
        encoded = body.encode()
        self.send_header("Content-Length", str(len(encoded)))
        self.end_headers()
        self.wfile.write(encoded)


class _Rejected(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


def _parameters(params: dict) -> Parameters:
    defaults = asdict(Parameters())
    unknown = set(params) - set(defaults)
    if unknown:
        raise _Rejected("invalid_request", f"unknown parameters: {', '.join(sorted(unknown))}")
    return Parameters(**{**defaults, **params})


def fetch_container(url: str) -> bytes:
    """
    Fetch the container to solve against.

    Contract section 6 requires an absolute HTTPS URL. This example also accepts
    `http://localhost` and `http://127.0.0.1` so it can run against the local demo server;
    a deployed solver MUST NOT, and that is the access policy the contract asks it to document.
    """
    allowed = url.startswith(("https://", "http://localhost", "http://127.0.0.1"))
    if not allowed:
        raise _Rejected("invalid_request", "trama.url must be https, or http on localhost")
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read(MAXIMUM_CONTAINER_BYTES + 1)[: MAXIMUM_CONTAINER_BYTES + 1]


def serve(port: int = 8801) -> None:
    server = ThreadingHTTPServer(("127.0.0.1", port), SolveHandler)
    print(f"{SOLVER_ID} listening on http://127.0.0.1:{port}/solve")
    server.serve_forever()


if __name__ == "__main__":
    serve()
