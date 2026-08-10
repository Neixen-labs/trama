# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest

from trama_engine.compiler import compile_file, export_file
from trama_engine.reader import read_network, read_sections

_NETWORK = {
    "type": "FeatureCollection",
    "features": [
        {
            "type": "Feature",
            "id": "edge-a",
            "properties": {"material": "steel", "diameter": 0.3, "year": 1994, "closed": True},
            "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.7035, 40.4165]]},
        },
        {
            "type": "Feature",
            "id": "edge-b",
            "properties": {"material": "pvc", "diameter": 0.2, "closed": False},
            "geometry": {"type": "LineString", "coordinates": [[-3.7035, 40.4165], [-3.703, 40.417], [-3.7025, 40.417]]},
        },
    ],
}


def _compile(tmp_path: Path, document: dict = _NETWORK) -> Path:
    source = tmp_path / "network.geojson"
    source.write_text(json.dumps(document))
    destination = tmp_path / "network.trama"
    compile_file(source, destination)
    return destination


def _features(path: Path) -> list[dict]:
    return json.loads(path.read_text())["features"]


def test_export_writes_a_node_and_an_edge_collection(tmp_path: Path) -> None:
    nodes_path, edges_path = export_file(_compile(tmp_path), tmp_path / "out", "geojson")

    assert nodes_path.name == "out.nodes.geojson"
    assert edges_path.name == "out.edges.geojson"
    nodes = _features(nodes_path)
    edges = _features(edges_path)
    assert {feature["geometry"]["type"] for feature in nodes} == {"Point"}
    assert {feature["geometry"]["type"] for feature in edges} == {"LineString"}
    # Three nodes, because the two lines share an endpoint.
    assert len(nodes) == 3
    assert len(edges) == 2
    assert all(feature["properties"]["_trama_id"].isdigit() for feature in nodes + edges)


def test_export_returns_properties_with_their_types_intact(tmp_path: Path) -> None:
    _nodes_path, edges_path = export_file(_compile(tmp_path), tmp_path / "out", "geojson")

    by_material = {feature["properties"]["material"]: feature["properties"] for feature in _features(edges_path)}
    assert by_material["steel"] == {
        "_trama_id": by_material["steel"]["_trama_id"],
        "closed": True,
        "diameter": 0.3,
        "material": "steel",
        "year": 1994,
    }
    # An absent property stays absent, and false stays a value.
    assert "year" not in by_material["pvc"]
    assert by_material["pvc"]["closed"] is False


def test_export_coordinates_land_within_one_quantization_step(tmp_path: Path) -> None:
    _nodes_path, edges_path = export_file(_compile(tmp_path), tmp_path / "out", "geojson")

    exported = {
        len(feature["geometry"]["coordinates"]): feature["geometry"]["coordinates"]
        for feature in _features(edges_path)
    }
    for original, restored in zip(_NETWORK["features"][1]["geometry"]["coordinates"], exported[3]):
        assert original[0] == pytest.approx(restored[0], abs=1e-5)
        assert original[1] == pytest.approx(restored[1], abs=1e-5)


def test_round_trip_preserves_edge_identity_topology_and_properties(tmp_path: Path) -> None:
    first = _compile(tmp_path)
    _nodes_path, edges_path = export_file(first, tmp_path / "out", "geojson")
    second = tmp_path / "again.trama"
    compile_file(edges_path, second)

    before = read_network(first)
    after = read_network(second)
    assert [edge.id for edge in before.edges] == [edge.id for edge in after.edges]
    assert [edge.properties for edge in before.edges] == [edge.properties for edge in after.edges]
    # Node IDs derive from quantized position, so they change and the arrays reorder. What has to
    # survive is the topology: one consistent renaming of node indexes must explain every edge.
    assert len(before.nodes) == len(after.nodes)
    renaming: dict[int, int] = {}
    for edge_before, edge_after in zip(before.edges, after.edges):
        for old, new in ((edge_before.source, edge_after.source), (edge_before.target, edge_after.target)):
            assert renaming.setdefault(old, new) == new
    assert len(set(renaming.values())) == len(renaming) == len(before.nodes)


def test_round_trip_is_stable_after_the_first_export(tmp_path: Path) -> None:
    _nodes_path, edges_path = export_file(_compile(tmp_path), tmp_path / "out", "geojson")
    second = tmp_path / "again.trama"
    compile_file(edges_path, second)
    _again_nodes, again_edges = export_file(second, tmp_path / "again", "geojson")

    assert again_edges.read_text() == edges_path.read_text()


def test_compile_rejects_a_malformed_trama_id(tmp_path: Path) -> None:
    source = tmp_path / "broken.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"_trama_id":"not-a-number"},'
        '"geometry":{"type":"LineString","coordinates":[[0,0],[0.001,0]]}}]}'
    )

    with pytest.raises(ValueError, match="_trama_id must be a decimal u64 string"):
        compile_file(source, tmp_path / "broken.trama")


def test_reader_rejects_a_corrupted_section(tmp_path: Path) -> None:
    path = _compile(tmp_path)
    data = bytearray(path.read_bytes())
    graph_record = next(
        index for index in range(4) if data[64 + index * 64 : 64 + index * 64 + 4] == b"GRPH"
    )
    offset = struct.unpack_from("<Q", data, 64 + graph_record * 64 + 0x14)[0]
    data[offset + 8] ^= 0xFF
    path.write_bytes(data)

    with pytest.raises(ValueError):
        read_sections(path)


def test_reader_rejects_a_truncated_file(tmp_path: Path) -> None:
    path = _compile(tmp_path)
    path.write_bytes(path.read_bytes()[:-16])

    with pytest.raises(ValueError, match="file length disagrees"):
        read_sections(path)
