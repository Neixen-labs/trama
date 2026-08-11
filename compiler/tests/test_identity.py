# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest

from trama_engine.compiler import compile_geojson, parse_graph, read_sections
from trama_engine.exporter import export_geojson

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"


def _identities(container: Path) -> tuple[list[int], list[int]]:
    """Through parse_graph: SPEC 4.1 puts identity in a varint block, not at a fixed offset."""
    graph = next(payload for kind, _key, payload in read_sections(container.read_bytes()) if kind == b"GRPH")
    nodes, edges, _refs = parse_graph(graph)
    return ([node[0] for node in nodes], [edge[0] for edge in edges])


def test_recompiling_an_export_keeps_every_stable_id(tmp_path: Path) -> None:
    exported = tmp_path / "exported"
    recompiled = tmp_path / "recompiled.trama"

    export_geojson(FIXTURES / "network.trama", exported)
    compile_geojson(exported, recompiled)

    assert _identities(recompiled) == _identities(FIXTURES / "network.trama")


def test_a_declared_edge_id_wins_over_a_derived_one(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        json.dumps(
            {
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "id": "a",
                        "properties": {"_trama_id": "42", "loss": 1.5},
                        "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
                    }
                ],
            }
        )
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    _nodes, edges = _identities(destination)
    assert edges == [42]


def test_the_id_key_never_reaches_the_property_section(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","id":"a","properties":{"_trama_id":"42","loss":1.5},'
        '"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    properties = next(payload for kind, _key, payload in read_sections(destination.read_bytes()) if kind == b"PROP")
    assert struct.unpack_from("<I", properties)[0] == 1
    assert b"_trama_id" not in properties


@pytest.mark.parametrize("declared", ["not-a-number", "18446744073709551616", "-1"])
def test_a_malformed_id_is_rejected_rather_than_silently_replaced(tmp_path: Path, declared: str) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"_trama_id":"' + declared + '"},'
        '"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
    )

    with pytest.raises(ValueError, match="_trama_id must"):
        compile_geojson(source, tmp_path / "network.trama")


def test_compiling_a_directory_without_collections_is_rejected(tmp_path: Path) -> None:
    empty = tmp_path / "empty"
    empty.mkdir()

    with pytest.raises(ValueError, match="holds no edges.geojson"):
        compile_geojson(empty, tmp_path / "network.trama")
