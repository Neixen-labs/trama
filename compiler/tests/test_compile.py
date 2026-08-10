# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest
import zstandard

from trama_engine.compiler import compile_geojson, validate_container


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
