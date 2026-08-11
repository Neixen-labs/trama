# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The solver's deltas must be the toolkit's own numbers, keyed to stable identities."""

import struct
from pathlib import Path

import pytest
from epanet import toolkit as en
from trama_engine.compiler import compile_features

from trama_epanet.exporter import entity_ids
from trama_epanet.importer import EpanetImporter
from trama_epanet.solver import DELTA, InvalidInput, Parameters, solve

NETWORKS = Path(__file__).parent / "networks"
CRS = "EPSG:3857"


@pytest.fixture
def net3(tmp_path: Path) -> Path:
    imported = EpanetImporter().load(NETWORKS / "Net3.inp", {"source-crs": CRS})
    container = tmp_path / "net3.trama"
    compile_features(imported.features, container, imported.channels, imported.extras)
    return container


def _unpacked(deltas: bytes) -> list[tuple[int, int, float, float]]:
    assert len(deltas) % DELTA.size == 0
    return [DELTA.unpack_from(deltas, at) for at in range(0, len(deltas), DELTA.size)]


def _direct(source: Path, report: Path) -> dict[tuple[str, str, float], float]:
    project = en.createproject()
    en.open(project, str(source), str(report), "")
    en.openH(project)
    en.initH(project, en.SAVE)
    sampled = {}
    while True:
        now = float(en.runH(project))
        for index in range(1, en.getcount(project, en.NODECOUNT) + 1):
            sampled[("node", en.getnodeid(project, index), now)] = en.getnodevalue(project, index, en.PRESSURE)
        for index in range(1, en.getcount(project, en.LINKCOUNT) + 1):
            sampled[("link", en.getlinkid(project, index), now)] = en.getlinkvalue(project, index, en.FLOW)
        if en.nextH(project) == 0:
            break
    en.closeH(project)
    en.close(project)
    en.deleteproject(project)
    return sampled


def test_every_delta_matches_a_direct_run_of_the_source_network(net3: Path, tmp_path: Path) -> None:
    deltas = solve(net3.read_bytes(), Parameters(), 0.0, 86400.0)
    nodes, links = entity_ids(net3)
    identities = {identity: ("node", name) for name, identity in nodes.items()}
    identities |= {identity: ("link", name) for name, identity in links.items()}
    expected = _direct(NETWORKS / "Net3.inp", tmp_path / "direct.rpt")

    unpacked = _unpacked(deltas)
    assert unpacked, "the solver produced nothing"
    for entity_id, _channel, t, value in unpacked:
        kind, name = identities[entity_id]
        assert abs(value - expected[(kind, name, t)]) < 1e-3, f"{kind} {name} at {t}"


def test_pressure_and_flow_go_to_the_channels_the_container_declared(net3: Path) -> None:
    deltas = _unpacked(solve(net3.read_bytes(), Parameters(), 0.0, 3600.0))
    nodes, links = entity_ids(net3)

    written = {(entity_id, channel) for entity_id, channel, _t, _value in deltas}
    assert {channel for identity, channel in written if identity in nodes.values()} == {1}
    assert {channel for identity, channel in written if identity in links.values()} == {2}


def test_the_window_is_respected(net3: Path) -> None:
    early = _unpacked(solve(net3.read_bytes(), Parameters(), 0.0, 3600.0))
    late = _unpacked(solve(net3.read_bytes(), Parameters(), 7200.0, 10800.0))

    assert {t for _id, _channel, t, _value in early} <= {0.0, 3600.0}
    assert min(t for _id, _channel, t, _value in late) >= 7200.0
    assert max(t for _id, _channel, t, _value in late) <= 10800.0


def test_a_channel_the_container_never_declared_is_refused(net3: Path) -> None:
    with pytest.raises(InvalidInput, match="no node channel named 'head'"):
        solve(net3.read_bytes(), Parameters(pressure_channel="head"), 0.0, 3600.0)


def test_an_inverted_window_is_refused(net3: Path) -> None:
    with pytest.raises(InvalidInput, match="must not precede"):
        solve(net3.read_bytes(), Parameters(), 3600.0, 0.0)


def test_a_container_from_another_format_is_refused(tmp_path: Path) -> None:
    plain = tmp_path / "plain.trama"
    compile_features(
        [
            {
                "type": "Feature",
                "id": "a",
                "properties": {},
                "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
            }
        ],
        plain,
        [{"name": "pressure", "entity_kind": "node", "unit": "m"}, {"name": "flow", "entity_kind": "edge", "unit": "l/s"}],
    )

    with pytest.raises(InvalidInput, match="not compiled from an EPANET network"):
        solve(plain.read_bytes(), Parameters(), 0.0, 3600.0)


def test_the_delta_stream_is_a_whole_number_of_records(net3: Path) -> None:
    deltas = solve(net3.read_bytes(), Parameters(), 0.0, 3600.0)

    assert len(deltas) % 18 == 0
    assert struct.calcsize("<QHff") == 18
