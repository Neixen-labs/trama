# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest
import zstandard

from trama_engine.compiler import _stable_id, compile_geojson, validate_container

_LINES = {
    "a": [[-3.704, 40.416], [-3.703, 40.417]],
    "b": [[-3.703, 40.417], [-3.702, 40.418]],
    "c": [[-3.702, 40.418], [-3.701, 40.419]],
}


def _compile_features(tmp_path: Path, properties: dict[str, dict[str, object]]) -> Path:
    source = tmp_path / "network.geojson"
    source.write_text(
        json.dumps(
            {
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "id": feature_id,
                        "properties": properties[feature_id],
                        "geometry": {"type": "LineString", "coordinates": _LINES[feature_id]},
                    }
                    for feature_id in properties
                ],
            }
        )
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)
    return destination


def _edge_order(feature_ids: list[str]) -> list[str]:
    return sorted(feature_ids, key=lambda feature_id: _stable_id(f"edge:{feature_id}"))


def _decode_property_section(destination: Path) -> bytes:
    data = destination.read_bytes()
    record = 64 + 2 * 64
    offset = struct.unpack_from("<Q", data, record + 20)[0]
    stored_bytes = struct.unpack_from("<Q", data, record + 36)[0]
    return zstandard.ZstdDecompressor().decompress(data[offset : offset + stored_bytes])


def _first_column(prop: bytes) -> tuple[int, int, int, int, int, int, int]:
    columns_offset = struct.unpack_from("<I", prop, 36)[0]
    return struct.unpack_from("<IBBHIII", prop, columns_offset)


def test_compile_geojson_writes_deterministic_v0_container(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        json.dumps(
            {
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "id": "edge-a",
                        "properties": {},
                        "geometry": {
                            "type": "LineString",
                            "coordinates": [[-3.704, 40.416], [-3.703, 40.417]],
                        },
                    }
                ],
            }
        )
    )
    first = tmp_path / "first.trama"
    second = tmp_path / "second.trama"

    compile_geojson(source, first)
    compile_geojson(source, second)

    data = first.read_bytes()
    assert data == second.read_bytes()
    assert data[:8] == b"TRAMA\0\0\0"
    assert struct.unpack_from("<HHH", data, 8) == (0, 1, 0)
    assert struct.unpack_from("<I", data, 0x20)[0] == 4
    directory_offset = struct.unpack_from("<Q", data, 0x18)[0]
    section_types = [
        data[directory_offset + index * 64 : directory_offset + index * 64 + 4]
        for index in range(4)
    ]
    assert section_types == [b"GEOM", b"GRPH", b"PROP", b"STCH"]


def test_compile_geojson_writes_typed_edge_properties(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{"active":true,"label":"main","loss":1.5,"rank":3},"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    data = destination.read_bytes()
    prop_record = 64 + 2 * 64
    prop_offset = struct.unpack_from("<Q", data, prop_record + 20)[0]
    prop_size = struct.unpack_from("<Q", data, prop_record + 36)[0]
    prop = zstandard.ZstdDecompressor().decompress(data[prop_offset : prop_offset + prop_size])
    assert struct.unpack_from("<5I", prop) == (4, 1, 0, 0, 4)
    assert b"active" in prop and b"label" in prop and b"loss" in prop and b"rank" in prop


def test_compile_geojson_rejects_lines_that_span_tiles(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[0,0],[1,0]]}}]}'
    )

    with pytest.raises(ValueError, match="does not support lines spanning tiles"):
        compile_geojson(source, tmp_path / "network.trama")


def test_compile_geojson_sorts_nodes_by_stable_id(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","id":"edge-a","properties":{},"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)

    data = destination.read_bytes()
    graph_offset = struct.unpack_from("<Q", data, 64 + 64 + 20)[0]
    graph_size = struct.unpack_from("<Q", data, 64 + 64 + 36)[0]
    graph = zstandard.ZstdDecompressor().decompress(data[graph_offset : graph_offset + graph_size])
    nodes_offset = struct.unpack_from("<I", graph, 16)[0]
    node_ids = [struct.unpack_from("<Q", graph, nodes_offset + index * 16)[0] for index in range(2)]
    assert node_ids == sorted(node_ids)


def test_validate_container_rejects_corrupted_section(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)
    damaged = bytearray(destination.read_bytes())
    damaged[-1] ^= 1
    destination.write_bytes(damaged)

    with pytest.raises(ValueError, match="invalid section integrity"):
        validate_container(destination)


def test_compile_geojson_connects_multiple_lines_at_shared_endpoints(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":['
        '{"type":"Feature","id":"a","properties":{},"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}},'
        '{"type":"Feature","id":"b","properties":{},"geometry":{"type":"LineString","coordinates":[[-3.703,40.417],[-3.702,40.418]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    data = destination.read_bytes()
    graph_record = 64 + 64
    graph_offset = struct.unpack_from("<Q", data, graph_record + 20)[0]
    graph_size = struct.unpack_from("<Q", data, graph_record + 36)[0]
    graph = zstandard.ZstdDecompressor().decompress(data[graph_offset : graph_offset + graph_size])
    assert struct.unpack_from("<3I", graph) == (3, 2, 4)


def test_compile_geojson_marks_absent_edge_values_in_the_presence_bitmap(tmp_path: Path) -> None:
    destination = _compile_features(tmp_path, {"a": {"loss": 1.5}, "b": {}, "c": {"loss": 2.5}})

    prop = _decode_property_section(destination)
    order = _edge_order(["a", "b", "c"])
    assert struct.unpack_from("<5I", prop) == (1, 0, 0, 0, 1)
    _key_id, kind, value_type, flags, entity_count, presence_offset, values_offset = _first_column(prop)
    assert (kind, value_type, flags, entity_count) == (2, 1, 1, 3)
    assert prop[presence_offset] == sum(1 << index for index, feature_id in enumerate(order) if feature_id != "b")
    present = {"a": 1.5, "c": 2.5}
    assert [
        struct.unpack_from("<d", prop, values_offset + index * 8)[0] for index in range(2)
    ] == [present[feature_id] for feature_id in order if feature_id in present]


def test_compile_geojson_promotes_mixed_integer_and_float_columns_to_f64(tmp_path: Path) -> None:
    destination = _compile_features(tmp_path, {"a": {"rank": 3}, "b": {"rank": 2.5}})

    prop = _decode_property_section(destination)
    order = _edge_order(["a", "b"])
    _key_id, _kind, value_type, _flags, entity_count, presence_offset, values_offset = _first_column(prop)
    assert (value_type, entity_count, prop[presence_offset]) == (1, 2, 0b11)
    ranks = {"a": 3.0, "b": 2.5}
    assert [
        struct.unpack_from("<d", prop, values_offset + index * 8)[0] for index in range(2)
    ] == [ranks[feature_id] for feature_id in order]


def test_compile_geojson_rejects_conflicting_property_types(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="conflicting types"):
        _compile_features(tmp_path, {"a": {"rank": 3}, "b": {"rank": "high"}})


def test_compile_geojson_packs_boolean_columns_as_bits(tmp_path: Path) -> None:
    destination = _compile_features(tmp_path, {"a": {"open": True}, "b": {}, "c": {"open": False}})

    prop = _decode_property_section(destination)
    order = _edge_order(["a", "b", "c"])
    _key_id, _kind, value_type, _flags, entity_count, presence_offset, values_offset = _first_column(prop)
    assert (value_type, entity_count) == (4, 3)
    assert prop[presence_offset] == sum(1 << index for index, feature_id in enumerate(order) if feature_id != "b")
    assert prop[values_offset] == (0b01 if order.index("a") < order.index("c") else 0b10)


def test_compile_geojson_points_each_edge_at_its_property_row(tmp_path: Path) -> None:
    destination = _compile_features(tmp_path, {"a": {"loss": 1.5}, "b": {}, "c": {"loss": 2.5}})

    data = destination.read_bytes()
    graph_record = 64 + 64
    graph_offset = struct.unpack_from("<Q", data, graph_record + 20)[0]
    graph_size = struct.unpack_from("<Q", data, graph_record + 36)[0]
    graph = zstandard.ZstdDecompressor().decompress(data[graph_offset : graph_offset + graph_size])
    edges_offset = struct.unpack_from("<I", graph, 20)[0]
    property_rows = [struct.unpack_from("<I", graph, edges_offset + index * 32 + 16)[0] for index in range(3)]
    assert property_rows == [0, 1, 2]
