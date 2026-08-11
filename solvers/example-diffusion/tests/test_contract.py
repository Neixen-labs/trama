# SPDX-License-Identifier: LicenseRef-BSL-1.1
import base64
import json
import struct
import threading
import urllib.request
from collections.abc import Iterator
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

from example_diffusion.server import CONTRACT_VERSION, SolveHandler
from example_diffusion.solver import DELTA

FIXTURES = Path(__file__).resolve().parents[3] / "fixtures"


@pytest.fixture(scope="module")
def containers() -> Iterator[str]:
    """Serves fixtures/ so the solver fetches a real container over HTTP, as the contract says."""
    handler = type("Handler", (SimpleHTTPRequestHandler,), {"directory": str(FIXTURES)})
    server = ThreadingHTTPServer(("127.0.0.1", 0), lambda *args: handler(*args, directory=str(FIXTURES)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


@pytest.fixture(scope="module")
def solver() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), SolveHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/solve"
    server.shutdown()


def _post(url: str, body: dict) -> list[tuple[str, str]]:
    request = urllib.request.Request(
        url, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST"
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        stream = response.read().decode()
    events = []
    for block in stream.split("\n\n"):
        if not block.strip():
            continue
        name = next(line[len("event: ") :] for line in block.splitlines() if line.startswith("event: "))
        data = next(line[len("data: ") :] for line in block.splitlines() if line.startswith("data: "))
        events.append((name, data))
    return events


def _request(containers: str, **overrides: object) -> dict:
    return {
        "contract_version": CONTRACT_VERSION,
        "trama": {"url": f"{containers}/demo-grid.trama"},
        "params": {},
        "t0_seconds": 0,
        "t1_seconds": 300,
        **overrides,
    }


def _deltas(events: list[tuple[str, str]]) -> list[tuple[int, int, float, float]]:
    payload = b"".join(base64.b64decode(data) for name, data in events if name == "delta")
    assert len(payload) % DELTA.size == 0, "a delta stream must be whole 18-byte records"
    return [DELTA.unpack_from(payload, at) for at in range(0, len(payload), DELTA.size)]


def test_a_solve_streams_ready_deltas_and_complete(containers: str, solver: str) -> None:
    events = _post(solver, _request(containers))

    assert events[0][0] == "ready"
    assert json.loads(events[0][1]) == {"contract_version": CONTRACT_VERSION, "solver_id": "example-diffusion"}
    assert events[-1][0] == "complete"
    assert {name for name, _data in events[1:-1]} == {"delta"}
    assert json.loads(events[-1][1])["delta_count"] == len(_deltas(events))


def test_deltas_are_ordered_by_time_then_channel_then_entity(containers: str, solver: str) -> None:
    deltas = _deltas(_post(solver, _request(containers)))

    keys = [(t, channel, entity) for entity, channel, t, _value in deltas]
    assert keys == sorted(keys)
    assert len(keys) == len(set(keys)), "a repeated (entity, channel, t) makes the stream invalid"


def test_every_value_respects_the_declared_range(containers: str, solver: str) -> None:
    deltas = _deltas(_post(solver, _request(containers)))

    # demo-grid.trama declares flow over [-50, 50].
    assert all(-50 <= value <= 50 for _entity, _channel, _t, value in deltas)
    assert any(value > 1 for _entity, _channel, _t, value in deltas), "the pulse must actually reach some edges"


def test_the_same_request_twice_produces_identical_bytes(containers: str, solver: str) -> None:
    first = b"".join(base64.b64decode(data) for name, data in _post(solver, _request(containers)) if name == "delta")
    second = b"".join(base64.b64decode(data) for name, data in _post(solver, _request(containers)) if name == "delta")

    assert first == second


@pytest.mark.parametrize(
    ("overrides", "code"),
    [
        ({"contract_version": "9.9.9"}, "unsupported_contract"),
        ({"trama": {}}, "invalid_request"),
        ({"params": {"nonsense": 1}}, "invalid_request"),
        ({"params": {"channel": "no-such-channel"}}, "invalid_input"),
        ({"t0_seconds": 100, "t1_seconds": 0}, "invalid_input"),
    ],
)
def test_a_rejected_request_sends_one_terminal_error(
    containers: str, solver: str, overrides: dict, code: str
) -> None:
    with pytest.raises(urllib.error.HTTPError) as rejection:
        _post(solver, _request(containers, **overrides))

    stream = rejection.value.read().decode()
    assert stream.count("event: ") == 1
    assert stream.startswith("event: error")
    assert json.loads(stream.split("data: ", 1)[1])["code"] == code


def test_a_container_that_cannot_be_fetched_is_reported_as_such(containers: str, solver: str) -> None:
    with pytest.raises(urllib.error.HTTPError) as rejection:
        _post(solver, _request(containers, trama={"url": f"{containers}/missing.trama"}))

    assert json.loads(rejection.value.read().decode().split("data: ", 1)[1])["code"] == "fetch_failed"


def test_a_non_local_plain_http_url_is_refused(solver: str) -> None:
    with pytest.raises(urllib.error.HTTPError) as rejection:
        _post(solver, _request("http://example.invalid"))

    assert json.loads(rejection.value.read().decode().split("data: ", 1)[1])["code"] == "invalid_request"


def test_the_manifest_declares_what_the_solver_writes() -> None:
    import tomllib

    manifest = tomllib.loads((Path(__file__).resolve().parents[1] / "solver.toml").read_text())

    assert manifest["contract_versions"] == [CONTRACT_VERSION]
    assert manifest["runtimes"] == ["server"]
    assert manifest["outputs"] == [{"channel": "flow", "entity_kind": "edge", "unit": "l/s"}]
    assert struct.calcsize("<QHff") == 18
