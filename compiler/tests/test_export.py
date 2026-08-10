# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
from pathlib import Path

import pytest

from trama_engine.compiler import _stable_id
from trama_engine.exporter import export_geojson

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures"
# One z14 tile is ~2446 m wide over 65535 steps, so a coordinate returns within ~4 cm.
QUANTIZATION_DEGREES = 1e-6


def _export(tmp_path: Path) -> tuple[dict[str, dict], dict[str, dict]]:
    destination = tmp_path / "out"
    export_geojson(FIXTURES / "network.trama", destination)
    return (
        {
            feature["properties"]["_trama_id"]: feature
            for feature in json.loads((destination / "nodes.geojson").read_text())["features"]
        },
        {
            feature["properties"]["_trama_id"]: feature
            for feature in json.loads((destination / "edges.geojson").read_text())["features"]
        },
    )


def test_export_returns_every_edge_with_its_endpoints_and_properties(tmp_path: Path) -> None:
    source = json.loads((FIXTURES / "network.geojson").read_text())
    _nodes, edges = _export(tmp_path)

    assert len(edges) == len(source["features"])
    for feature in source["features"]:
        exported = edges[str(_stable_id(f"edge:{feature['id']}"))]
        original = feature["geometry"]["coordinates"]
        coordinates = exported["geometry"]["coordinates"]
        assert coordinates[0] == pytest.approx(original[0], abs=QUANTIZATION_DEGREES)
        assert coordinates[-1] == pytest.approx(original[-1], abs=QUANTIZATION_DEGREES)
        assert {key: value for key, value in exported["properties"].items() if key != "_trama_id"} == feature[
            "properties"
        ]


def test_export_rejoins_an_edge_that_was_split_across_tiles(tmp_path: Path) -> None:
    _nodes, edges = _export(tmp_path)
    trunk = edges[str(_stable_id("edge:trunk"))]

    assert trunk["geometry"]["type"] == "LineString"
    # Two source vertices plus the vertex the compiler inserted at the tile boundary.
    assert len(trunk["geometry"]["coordinates"]) == 3
    longitudes = [longitude for longitude, _latitude in trunk["geometry"]["coordinates"]]
    assert longitudes == sorted(longitudes)


def test_export_omits_absent_properties_rather_than_writing_null(tmp_path: Path) -> None:
    _nodes, edges = _export(tmp_path)

    assert set(edges[str(_stable_id("edge:spur"))]["properties"]) == {"_trama_id"}
    assert "rank" not in edges[str(_stable_id("edge:branch"))]["properties"]
    assert edges[str(_stable_id("edge:branch"))]["properties"]["loss"] == 2.5


def test_export_writes_one_point_per_node(tmp_path: Path) -> None:
    nodes, edges = _export(tmp_path)

    assert len(nodes) == 4
    assert all(feature["geometry"]["type"] == "Point" for feature in nodes.values())
    endpoints = {
        tuple(edge["geometry"]["coordinates"][index]) for edge in edges.values() for index in (0, -1)
    }
    assert {tuple(node["geometry"]["coordinates"]) for node in nodes.values()} == endpoints
