# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Point features carry typed properties into node columns, per SPEC 5."""

import json
import struct
from pathlib import Path

from trama_engine.compiler import compile_geojson, read_sections
from trama_engine.exporter import export_geojson

LINE = {
    "type": "Feature",
    "id": "a",
    "properties": {"loss": 1.5},
    "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
}


def _point(coordinates: list[float], properties: dict[str, object]) -> dict[str, object]:
    return {"type": "Feature", "properties": properties, "geometry": {"type": "Point", "coordinates": coordinates}}


def _compile(tmp_path: Path, features: list[dict[str, object]]) -> Path:
    source = tmp_path / "network.geojson"
    source.write_text(json.dumps({"type": "FeatureCollection", "features": features}))
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)
    return destination


def _columns(container: Path) -> tuple[int, int]:
    payload = next(p for kind, _key, p in read_sections(container.read_bytes()) if kind == b"PROP")
    _keys, _strings, _enums, node_columns, edge_columns = struct.unpack_from("<5I", payload)
    return node_columns, edge_columns


def test_point_properties_become_node_columns(tmp_path: Path) -> None:
    container = _compile(
        tmp_path,
        [LINE, _point([-3.704, 40.416], {"elevation": 710.0, "name": "J-10"})],
    )

    assert _columns(container) == (2, 1)


def test_a_node_without_a_point_feature_has_no_values_rather_than_zeros(tmp_path: Path) -> None:
    container = _compile(tmp_path, [LINE, _point([-3.704, 40.416], {"elevation": 710.0})])

    exported = tmp_path / "exported"
    export_geojson(container, exported)
    nodes = json.loads((exported / "nodes.geojson").read_text())["features"]
    described = {feature["properties"].get("elevation") for feature in nodes}
    assert described == {710.0, None}  # absence is not 0.0


def test_node_properties_survive_export_and_recompilation(tmp_path: Path) -> None:
    container = _compile(
        tmp_path,
        [LINE, _point([-3.704, 40.416], {"elevation": 710.0, "kind": "junction", "closed": False})],
    )
    exported = tmp_path / "exported"
    export_geojson(container, exported)
    recompiled = tmp_path / "again.trama"

    compile_geojson(exported, recompiled)

    assert _columns(recompiled) == _columns(container)
    again = tmp_path / "again"
    export_geojson(recompiled, again)
    assert json.loads((again / "nodes.geojson").read_text()) == json.loads((exported / "nodes.geojson").read_text())


def test_a_point_property_does_not_leak_into_the_edge_columns(tmp_path: Path) -> None:
    container = _compile(tmp_path, [LINE, _point([-3.704, 40.416], {"elevation": 710.0})])

    exported = tmp_path / "exported"
    export_geojson(container, exported)
    edges = json.loads((exported / "edges.geojson").read_text())["features"]
    assert [feature["properties"].get("loss") for feature in edges] == [1.5]
    assert all("elevation" not in feature["properties"] for feature in edges)
