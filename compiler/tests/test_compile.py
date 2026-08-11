# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest
import zstandard

from trama_engine.compiler import (
    _stable_id,
    compile_geojson,
    parse_graph,
    validate_container,
)

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


def test_compile_geojson_keeps_every_piece_inside_its_own_tile(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","properties":{},"geometry":{"type":"LineString","coordinates":[[0,0],[1,0.5]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    tiles = _tile_keys(destination)
    assert len(tiles) > 1 and len(set(tiles)) == len(tiles)
    assert sum(len(_paths(destination, index)) for index in range(len(tiles))) == len(
        _geometry_refs(destination, len(tiles))
    )
    validate_container(destination)


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
    node_ids = [node[0] for node in parse_graph(graph)[0]]
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
    property_rows = [edge[3] for edge in parse_graph(graph)[1]]
    assert property_rows == [0, 1, 2]


def _decode_section(destination: Path, index: int) -> bytes:
    data = destination.read_bytes()
    record = 64 + index * 64
    offset = struct.unpack_from("<Q", data, record + 20)[0]
    stored_bytes = struct.unpack_from("<Q", data, record + 36)[0]
    return zstandard.ZstdDecompressor().decompress(data[offset : offset + stored_bytes])


def _section_kinds(destination: Path) -> list[bytes]:
    data = destination.read_bytes()
    count = struct.unpack_from("<I", data, 0x20)[0]
    return [data[64 + index * 64 : 64 + index * 64 + 4] for index in range(count)]


def _tile_keys(destination: Path) -> list[tuple[int, int, int]]:
    data = destination.read_bytes()
    return [
        struct.unpack_from("<3I", data, 64 + index * 64 + 8)
        for index, kind in enumerate(_section_kinds(destination))
        if kind == b"GEOM"
    ]


def _geometry_refs(destination: Path, graph_index: int) -> list[tuple[int, int, int]]:
    graph = _decode_section(destination, graph_index)
    count, refs_offset = struct.unpack_from("<I", graph, 12)[0], struct.unpack_from("<I", graph, 32)[0]
    return [struct.unpack_from("<IIb", graph, refs_offset + index * 12) for index in range(count)]


def _paths(destination: Path, geometry_index: int) -> list[tuple[int, list[tuple[int, int]]]]:
    geometry = _decode_section(destination, geometry_index)
    path_count, _vertices, _mesh, _indices, paths_offset, vertices_offset = struct.unpack_from("<6I", geometry)
    paths = []
    for index in range(path_count):
        edge_index, first_vertex, vertex_count, _flags = struct.unpack_from("<4I", geometry, paths_offset + index * 16)
        paths.append(
            (
                edge_index,
                [
                    struct.unpack_from("<HH", geometry, vertices_offset + (first_vertex + offset) * 4)
                    for offset in range(vertex_count)
                ],
            )
        )
    return paths


def test_compile_geojson_splits_a_line_at_a_tile_boundary(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","id":"a","properties":{},'
        '"geometry":{"type":"LineString","coordinates":[[-3.6700,40.416],[-3.6690,40.416]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    assert _section_kinds(destination) == [b"GEOM", b"GEOM", b"GRPH", b"PROP", b"STCH"]
    left, right = _tile_keys(destination)
    assert (left[0], right[0]) == (14, 14) and left[2] == right[2] and left[1] + 1 == right[1]
    assert _geometry_refs(destination, 2) == [(0, 0, 1), (1, 0, 1)]
    assert _paths(destination, 0)[0][0] == 0 and _paths(destination, 1)[0][0] == 0
    assert _paths(destination, 0)[0][1][-1][0] == 65535
    assert _paths(destination, 1)[0][1][0][0] == 0
    validate_container(destination)


def test_compile_geojson_orders_edge_pieces_by_traversal(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","id":"a","properties":{},'
        '"geometry":{"type":"LineString","coordinates":[[-3.66,40.416],[-3.75,40.416]]}}]}'
    )
    destination = tmp_path / "network.trama"

    compile_geojson(source, destination)

    tiles = _tile_keys(destination)
    refs = _geometry_refs(destination, len(tiles))
    assert len(tiles) > 2 and len(refs) == len(tiles)
    assert [tiles[directory_index][1] for directory_index, _path_index, _direction in refs] == sorted(
        (tile[1] for tile in tiles), reverse=True
    )
    assert all(direction == 1 for *_rest, direction in refs)


def test_compile_geojson_stays_deterministic_across_tiles(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":['
        '{"type":"Feature","id":"a","properties":{"loss":1.5},"geometry":{"type":"LineString","coordinates":[[-3.75,40.416],[-3.66,40.42]]}},'
        '{"type":"Feature","id":"b","properties":{},"geometry":{"type":"LineString","coordinates":[[-3.66,40.42],[-3.60,40.44]]}}]}'
    )
    first = tmp_path / "first.trama"
    second = tmp_path / "second.trama"

    compile_geojson(source, first)
    compile_geojson(source, second)

    assert first.read_bytes() == second.read_bytes()
    validate_container(first)


def test_compile_geojson_writes_no_mesh_for_lines(tmp_path: Path) -> None:
    destination = _compile_features(tmp_path, {"a": {}, "b": {}})

    geometry = _decode_section(destination, 0)
    path_count, vertex_count, mesh_vertex_count, mesh_index_count = struct.unpack_from("<4I", geometry)
    _paths_offset, vertices_offset, mesh_vertices_offset, mesh_indices_offset = struct.unpack_from("<4I", geometry, 16)
    assert (mesh_vertex_count, mesh_index_count) == (0, 0)
    assert mesh_vertices_offset == mesh_indices_offset == vertices_offset + vertex_count * 4 == len(geometry)
    assert path_count == 2


def test_shared_fixture_matches_a_fresh_compile(tmp_path: Path) -> None:
    fixtures = Path(__file__).resolve().parents[2] / "fixtures"
    destination = tmp_path / "network.trama"

    compile_geojson(fixtures / "network.geojson", destination)

    assert destination.read_bytes() == (fixtures / "network.trama").read_bytes(), (
        "regenerate fixtures/network.trama: the engine round-trip test reads it"
    )
