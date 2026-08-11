# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The server runtime, exercised over a real socket rather than a fake of the protocol."""

import base64
import json
import threading
import urllib.error
import urllib.request
from collections.abc import Iterator
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
from trama_engine.compiler import compile_features

from trama_epanet.importer import EpanetImporter
from trama_epanet.server import CONTRACT_VERSION, SOLVER_ID, SolveHandler

NETWORKS = Path(__file__).parent / "networks"


@pytest.fixture
def container_url(tmp_path: Path) -> Iterator[str]:
    imported = EpanetImporter().load(NETWORKS / "Net1.inp", {"source-crs": "EPSG:3857"})
    compile_features(imported.features, tmp_path / "net1.trama", imported.channels, imported.extras)
    files = ThreadingHTTPServer(("127.0.0.1", 0), partial(SimpleHTTPRequestHandler, directory=str(tmp_path)))
    threading.Thread(target=files.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{files.server_address[1]}/net1.trama"
    files.shutdown()


@pytest.fixture
def solver() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), SolveHandler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    yield f"http://127.0.0.1:{server.server_address[1]}/solve"
    server.shutdown()


def _post(endpoint: str, body: dict) -> list[tuple[str, str]]:
    request = urllib.request.Request(
        endpoint, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        return _events(response.read().decode())


def _events(stream: str) -> list[tuple[str, str]]:
    events = []
    for block in stream.split("\n\n"):
        lines = block.splitlines()
        if len(lines) >= 2:
            events.append((lines[0].removeprefix("event: "), lines[1].removeprefix("data: ")))
    return events


def test_a_solve_streams_deltas_and_ends_with_complete(solver: str, container_url: str) -> None:
    events = _post(
        solver,
        {"contract_version": CONTRACT_VERSION, "trama": {"url": container_url}, "t0_seconds": 0, "t1_seconds": 7200},
    )

    assert events[0][0] == "ready"
    assert json.loads(events[0][1])["solver_id"] == SOLVER_ID
    assert events[-1][0] == "complete"
    delivered = sum(len(base64.b64decode(data)) for name, data in events if name == "delta")
    assert delivered == json.loads(events[-1][1])["delta_count"] * 18


def test_an_undeclared_channel_becomes_one_error_event(solver: str, container_url: str) -> None:
    with pytest.raises(urllib.error.HTTPError) as raised:
        _post(
            solver,
            {
                "contract_version": CONTRACT_VERSION,
                "trama": {"url": container_url},
                "params": {"flow_channel": "velocity"},
                "t0_seconds": 0,
                "t1_seconds": 60,
            },
        )

    events = _events(raised.value.read().decode())
    assert [name for name, _data in events] == ["error"]
    assert json.loads(events[0][1])["code"] == "invalid_input"


def test_another_contract_version_is_refused_before_any_work(solver: str, container_url: str) -> None:
    with pytest.raises(urllib.error.HTTPError) as raised:
        _post(solver, {"contract_version": "9.9.9", "trama": {"url": container_url}})

    assert json.loads(_events(raised.value.read().decode())[0][1])["code"] == "unsupported_contract"
