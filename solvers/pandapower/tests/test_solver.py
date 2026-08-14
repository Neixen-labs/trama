# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""What the fourth domain must survive: the container being the whole input.

The fixture is compiled by `trama compile --importer power` from the same pandapower network
these tests load directly, so the comparison is against the source of truth rather than against
a recorded answer.
"""

from __future__ import annotations

import json
import struct
import threading
import urllib.error
import urllib.request
from http.server import HTTPServer
from pathlib import Path

import pandapower
import pandapower.networks
import pytest

from trama_pandapower.container import Container, MalformedContainer, crc32c
from trama_pandapower.server import Handler, Refused, run
from trama_pandapower.solver import Unsolvable, rebuild, solve

FIXTURE = Path(__file__).resolve().parents[3] / "fixtures" / "oberrhein.trama"
DELTA = struct.Struct("<QHff")


@pytest.fixture(scope="session")
def container() -> Container:
    return Container(FIXTURE.read_bytes())


def records(deltas: bytes) -> list[tuple[int, int, float, float]]:
    return [DELTA.unpack_from(deltas, at) for at in range(0, len(deltas), DELTA.size)]


def test_crc32c_is_castagnoli_and_not_the_other_crc32():
    # The check value every CRC-32C implementation is measured against.
    assert crc32c(b"123456789") == 0xE3069283


def test_the_container_decodes_from_the_specification_alone(container: Container):
    graph = container.graph()
    node_properties, edge_properties = container.properties()

    assert len(graph.node_ids) == 179
    assert len(graph.edge_ids) == 183
    # SPEC 4: both arrays are sorted by ascending stable id, which is what makes 4.1's deltas work.
    assert graph.node_ids == sorted(graph.node_ids)
    assert graph.edge_ids == sorted(graph.edge_ids)
    assert {channel.name for channel in container.channels()} == {"voltage", "loading"}
    assert node_properties["power:kind"][0] == "bus"
    # Absence is distinct from zero: a trafo has no length, and a line has no tap position.
    assert any(value is None for value in edge_properties["power:length_km"])


def test_the_rebuilt_network_is_the_one_that_was_compiled(container: Container):
    original = pandapower.networks.mv_oberrhein()

    rebuilt = rebuild(container).net

    for table in ("bus", "line", "trafo", "load", "ext_grid", "switch"):
        assert len(getattr(rebuilt, table)) == len(getattr(original, table)), table
    # Not just the counts: the electrical parameters came back through PROP as themselves.
    assert rebuilt.line.r_ohm_per_km.sum() == pytest.approx(original.line.r_ohm_per_km.sum())
    assert sorted(rebuilt.bus.index) == sorted(original.bus.index)


def test_the_load_flow_matches_pandapower_over_the_source_network(container: Container):
    original = pandapower.networks.mv_oberrhein()
    pandapower.runpp(original)
    rebuilt = rebuild(container)

    found = {(entity, channel): value for entity, channel, _t, value in records(solve(container, 0.0, 0.0, {}))}
    voltage, loading = (channel.channel_id for channel in sorted(container.channels(), key=lambda c: c.entity_kind))

    # f32 in the delta is the only thing between these two numbers.
    for index, expected in original.res_bus.vm_pu.items():
        assert found[(rebuilt.bus_ids[index], voltage)] == pytest.approx(float(expected), abs=1e-6)
    for table, kind in (("res_line", "line"), ("res_trafo", "trafo")):
        for index, expected in getattr(original, table).loading_percent.items():
            assert found[(rebuilt.element_ids[(kind, index)], loading)] == pytest.approx(float(expected), abs=1e-4)


def test_a_load_flow_is_one_instant_unless_the_caller_supplies_a_curve(container: Container):
    single = records(solve(container, 0.0, 3600.0, {}))
    assert {record[2] for record in single} == {0.0}

    # One real load flow per multiplier, spread across the interval — not an interpolation.
    series = records(solve(container, 0.0, 3600.0, {"load_scaling": [0.5, 1.0, 1.5]}))
    assert sorted({record[2] for record in series}) == [0.0, 1800.0, 3600.0]
    assert len(series) == 3 * len(single)
    # More load pulls the network down: the worst voltage at 1.5x is below the worst at 0.5x.
    worst = {t: min(value for _e, ch, at, value in series if at == t and ch == 1) for t in (0.0, 3600.0)}
    assert worst[3600.0] < worst[0.0]


def test_deltas_are_ordered_as_the_contract_requires(container: Container):
    ordered = records(solve(container, 0.0, 60.0, {"load_scaling": [1.0, 1.1]}))

    keys = [(record[2], record[1], record[0]) for record in ordered]
    assert keys == sorted(keys)
    # And no duplicate (entity, channel, t), which the contract calls an invalid stream.
    assert len(set(keys)) == len(keys)


def test_a_container_from_another_domain_is_refused():
    water = Path(__file__).resolve().parents[3] / "fixtures" / "net3.trama"

    with pytest.raises(Unsolvable, match="not compiled from a pandapower network"):
        solve(Container(water.read_bytes()), 0.0, 0.0, {})


def test_a_truncated_container_is_refused_rather_than_half_read():
    with pytest.raises(MalformedContainer):
        Container(FIXTURE.read_bytes()[:2048])


def test_the_server_refuses_a_contract_version_it_does_not_speak():
    with pytest.raises(Refused) as refusal:
        run({"contract_version": "9.9.9", "trama": {"url": "https://example.invalid/n.trama"}}, allow_http=False)

    assert refusal.value.code == "unsupported_contract"


def test_the_server_refuses_a_url_that_is_not_https():
    with pytest.raises(Refused) as refusal:
        run({"contract_version": "0.3.0", "trama": {"url": "http://example.invalid/n.trama"}}, allow_http=False)

    assert refusal.value.code == "invalid_request"
    # Even with --allow-http, only loopback: this must not become a way to reach an intranet.
    with pytest.raises(Refused) as elsewhere:
        run({"contract_version": "0.3.0", "trama": {"url": "http://10.0.0.1/n.trama"}}, allow_http=True)
    assert elsewhere.value.code == "invalid_request"


def test_the_server_streams_the_contract_events_end_to_end(tmp_path):
    """The whole path a client takes: fetch over HTTP, solve, read Server-Sent Events back."""
    files = _serve_directory(FIXTURE.parent)
    solver = _serve_solver()
    try:
        request = urllib.request.Request(
            f"http://127.0.0.1:{solver}/solve",
            data=json.dumps(
                {
                    "contract_version": "0.3.0",
                    "trama": {"url": f"http://127.0.0.1:{files}/oberrhein.trama"},
                    "params": {},
                    "t0_seconds": 0,
                    "t1_seconds": 0,
                }
            ).encode(),
            headers={"Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=120) as response:
            assert response.headers["Content-Type"] == "text/event-stream"
            stream = response.read().decode()
    finally:
        pass

    events = [block.split("\n", 1) for block in stream.strip().split("\n\n")]
    names = [event[0].removeprefix("event: ") for event in events]
    assert names[0] == "ready" and names[-1] == "complete"
    assert set(names[1:-1]) == {"delta"}
    assert json.loads(events[0][1].removeprefix("data: "))["solver_id"] == "pandapower"
    assert json.loads(events[-1][1].removeprefix("data: "))["delta_count"] == 362


def _serve_directory(directory: Path) -> int:
    from functools import partial
    from http.server import SimpleHTTPRequestHandler

    server = HTTPServer(("127.0.0.1", 0), partial(SimpleHTTPRequestHandler, directory=str(directory)))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]


def _serve_solver() -> int:
    Handler.allow_http = True
    server = HTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server.server_address[1]
