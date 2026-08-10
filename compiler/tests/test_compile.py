# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest
import zstandard

from trama_engine.compiler import _stable_id, compile_geojson

_DIRECTORY_OFFSET = 64
_DIRECTORY_SIZE = 64


def _line(coordinates: list[list[float]], identity: str | None = None, **properties: object) -> dict:
    feature = {
        "type": "Feature",
        "properties": properties,
        "geometry": {"type": "LineString", "coordinates": coordinates},
    }
    if identity is not None:
        feature["id"] = identity
    return feature


def _strings(payload: bytes, offset: int) -> list[str]:
    count = struct.unpack_from("<I", payload, offset)[0]
    values = []
    cursor = offset + 4
    for _ in range(count):
        size = struct.unpack_from("<I", payload, cursor)[0]
        values.append(payload[cursor + 4 : cursor + 4 + size].decode())
        cursor += 4 + size
    return values


def _edge_index(data: bytes, identity: str) -> int:
    """Resolve a source feature id to its entity index, which stable-ID ordering decides."""
    graph = next(payload for kind, _key, payload in _sections(data) if kind == b"GRPH")
    edge_count = struct.unpack_from("<I", graph, 4)[0]
    edges_offset = struct.unpack_from("<I", graph, 20)[0]
    ids = [struct.unpack_from("<Q", graph, edges_offset + index * 32)[0] for index in range(edge_count)]
    return ids.index(_stable_id(f"edge:{identity}"))


def _columns(payload: bytes) -> dict[str, tuple[int, bool, list[int], bytes]]:
    """Decode PROP into {key: (value_type, nullable, present entity indexes, value bytes)}."""
    key_count, _string_count, _enum_count, node_columns, edge_columns = struct.unpack_from("<5I", payload, 0)
    key_offset, _string_offset, _enum_offset, _node_offset, columns_offset = struct.unpack_from("<5I", payload, 20)
    keys = _strings(payload, key_offset)
    assert len(keys) == key_count
    decoded = {}
    for index in range(node_columns + edge_columns):
        key_id, kind, value_type, flags, entity_count, bitmap_offset, values_offset = struct.unpack_from(
            "<IBBHIII", payload, columns_offset + index * 20
        )
        assert kind == 2
        assert values_offset % 8 == 0
        present = [
            entity
            for entity in range(entity_count)
            if payload[bitmap_offset + entity // 8] >> (entity % 8) & 1
        ]
        decoded[keys[key_id]] = (value_type, bool(flags & 1), present, payload[values_offset:])
    return decoded


def _write(path: Path, *features: dict) -> Path:
    path.write_text(json.dumps({"type": "FeatureCollection", "features": list(features)}))
    return path


def _sections(data: bytes) -> list[tuple[bytes, tuple[int, int, int], bytes]]:
    count = struct.unpack_from("<I", data, 0x20)[0]
    sections = []
    for index in range(count):
        record = _DIRECTORY_OFFSET + index * _DIRECTORY_SIZE
        kind = data[record : record + 4]
        key = struct.unpack_from("<3I", data, record + 0x08)
        offset, stored = struct.unpack_from("<QQ", data, record + 0x14)
        sections.append((kind, key, zstandard.ZstdDecompressor().decompress(data[offset : offset + stored])))
    return sections


def test_compile_geojson_writes_deterministic_v0_container(tmp_path: Path) -> None:
    source = _write(tmp_path / "network.geojson", _line([[-3.704, 40.416], [-3.703, 40.417]], "edge-a"))
    first = tmp_path / "first.trama"
    second = tmp_path / "second.trama"

    compile_geojson(source, first)
    compile_geojson(source, second)

    data = first.read_bytes()
    assert data == second.read_bytes()
    assert data[:8] == b"TRAMA\0\0\0"
    assert struct.unpack_from("<HHH", data, 8) == (0, 1, 1)
    assert struct.unpack_from("<HHH", data, 14) == (0, 1, 0)
    assert struct.unpack_from("<Q", data, 0x28)[0] == len(data)
    assert [kind for kind, _key, _payload in _sections(data)] == [b"GEOM", b"GRPH", b"PROP", b"STCH"]


def test_compile_geojson_shares_a_node_between_touching_lines(tmp_path: Path) -> None:
    source = _write(
        tmp_path / "network.geojson",
        _line([[-3.704, 40.416], [-3.7035, 40.4165]], "edge-a"),
        _line([[-3.7035, 40.4165], [-3.703, 40.417]], "edge-b"),
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)

    graph = next(payload for kind, _key, payload in _sections(destination.read_bytes()) if kind == b"GRPH")
    node_count, edge_count, adjacency_count, _refs = struct.unpack_from("<4I", graph, 0)
    assert (node_count, edge_count, adjacency_count) == (3, 2, 4)

    nodes_offset, _edges_offset, csr_offset = struct.unpack_from("<3I", graph, 16)
    node_ids = [struct.unpack_from("<Q", graph, nodes_offset + index * 16)[0] for index in range(node_count)]
    assert node_ids == sorted(node_ids)

    csr = [struct.unpack_from("<Q", graph, csr_offset + index * 8)[0] for index in range(node_count + 1)]
    assert csr[0] == 0 and csr[-1] == adjacency_count
    assert csr == sorted(csr)
    # The shared endpoint is the only node carrying two adjacency entries.
    assert sorted(csr[index + 1] - csr[index] for index in range(node_count)) == [1, 1, 2]


def test_compile_geojson_drops_a_long_line_to_a_coarser_tile(tmp_path: Path) -> None:
    source = _write(tmp_path / "network.geojson", _line([[0, 0], [1, 0]], "edge-long"))
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)

    kind, (z, _x, _y), payload = _sections(destination.read_bytes())[0]
    assert kind == b"GEOM"
    assert z < 14
    path_count, path_vertex_count, mesh_vertex_count, mesh_index_count = struct.unpack_from("<4I", payload, 0)
    assert (path_count, path_vertex_count, mesh_vertex_count, mesh_index_count) == (1, 2, 0, 0)


def test_compile_geojson_gives_an_id_less_feature_an_order_independent_id(tmp_path: Path) -> None:
    forward = _write(tmp_path / "forward.geojson", _line([[0, 0], [0.001, 0]]), _line([[0.002, 0], [0.003, 0]]))
    reversed_order = _write(
        tmp_path / "reversed.geojson", _line([[0.002, 0], [0.003, 0]]), _line([[0, 0], [0.001, 0]])
    )
    first = tmp_path / "forward.trama"
    second = tmp_path / "reversed.trama"

    compile_geojson(forward, first)
    compile_geojson(reversed_order, second)

    assert first.read_bytes() == second.read_bytes()


def test_compile_geojson_encodes_one_typed_column_per_property_key(tmp_path: Path) -> None:
    source = _write(
        tmp_path / "network.geojson",
        _line([[0, 0], [0.001, 0]], "edge-a", material="steel", diameter=0.3, segments=4, closed=True),
        _line([[0.001, 0], [0.002, 0]], "edge-b", material="pvc", diameter=1, closed=False),
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)

    data = destination.read_bytes()
    payload = next(payload for kind, _key, payload in _sections(data) if kind == b"PROP")
    columns = _columns(payload)
    first = _edge_index(data, "edge-a")
    second = _edge_index(data, "edge-b")
    assert set(columns) == {"closed", "diameter", "material", "segments"}

    # An integer mixed with a float promotes the whole column to f64.
    value_type, nullable, present, values = columns["diameter"]
    assert (value_type, nullable, present) == (1, False, [0, 1])
    assert struct.unpack_from("<2d", values, 0)[first] == 0.3
    assert struct.unpack_from("<2d", values, 0)[second] == 1.0

    # A key missing from one feature makes its column nullable and its values dense.
    value_type, nullable, present, values = columns["segments"]
    assert (value_type, nullable, present) == (2, True, [first])
    assert struct.unpack_from("<q", values, 0)[0] == 4

    value_type, nullable, present, values = columns["material"]
    assert (value_type, nullable, present) == (3, False, [0, 1])
    dictionary = _strings(payload, struct.unpack_from("<5I", payload, 20)[1])
    labels = [dictionary[index] for index in struct.unpack_from("<2I", values, 0)]
    assert (labels[first], labels[second]) == ("steel", "pvc")

    # false is a value, not an absence.
    value_type, nullable, present, values = columns["closed"]
    assert (value_type, nullable, present) == (4, False, [0, 1])
    assert (values[0] >> first & 1, values[0] >> second & 1) == (1, 0)


def test_compile_geojson_treats_a_null_property_as_absent(tmp_path: Path) -> None:
    source = _write(
        tmp_path / "network.geojson",
        _line([[0, 0], [0.001, 0]], "edge-a", material=None),
        _line([[0.001, 0], [0.002, 0]], "edge-b", material="pvc"),
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)

    data = destination.read_bytes()
    payload = next(payload for kind, _key, payload in _sections(data) if kind == b"PROP")
    _value_type, nullable, present, _values = _columns(payload)["material"]
    assert (nullable, present) == (True, [_edge_index(data, "edge-b")])


def test_compile_geojson_rejects_a_property_that_mixes_types(tmp_path: Path) -> None:
    source = _write(
        tmp_path / "network.geojson",
        _line([[0, 0], [0.001, 0]], "edge-a", material="steel"),
        _line([[0.001, 0], [0.002, 0]], "edge-b", material=3),
    )

    with pytest.raises(ValueError, match="mixes value types"):
        compile_geojson(source, tmp_path / "network.trama")


def test_compile_geojson_rejects_a_nested_property_value(tmp_path: Path) -> None:
    source = _write(tmp_path / "network.geojson", _line([[0, 0], [0.001, 0]], "edge-a", tags=["a", "b"]))

    with pytest.raises(ValueError, match="unsupported value type: list"):
        compile_geojson(source, tmp_path / "network.trama")


def test_compile_geojson_rejects_duplicate_edge_identities(tmp_path: Path) -> None:
    source = _write(
        tmp_path / "network.geojson",
        _line([[0, 0], [0.001, 0]], "edge-a"),
        _line([[0.002, 0], [0.003, 0]], "edge-a"),
    )

    with pytest.raises(ValueError, match="duplicate edge identity"):
        compile_geojson(source, tmp_path / "network.trama")


def test_compile_geojson_rejects_a_document_without_lines(tmp_path: Path) -> None:
    source = _write(tmp_path / "network.geojson")

    with pytest.raises(ValueError, match="no LineString features"):
        compile_geojson(source, tmp_path / "network.trama")
