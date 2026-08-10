# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Minimal deterministic GeoJSON compiler for TRAMA v0."""

from __future__ import annotations

import hashlib
import json
import math
import struct
from pathlib import Path

import zstandard

_MAGIC = b"TRAMA\0\0\0"
_HEADER = struct.Struct("<8s3H3HIQIIQ16s")
_DIRECTORY = struct.Struct("<4sIIIIQQQIHBB12s")


def compile_geojson(source: Path, destination: Path) -> None:
    """Compile one LineString GeoJSON feature into a deterministic TRAMA file."""
    document = json.loads(source.read_text())
    features = document.get("features", [])
    if len(features) != 1 or features[0].get("geometry", {}).get("type") != "LineString":
        raise ValueError("v0 compiler slice requires one LineString feature")
    feature = features[0]
    properties = feature.get("properties") or {}
    if not isinstance(properties, dict):
        raise TypeError("GeoJSON properties must be an object")
    coordinates = feature["geometry"].get("coordinates", [])
    if len(coordinates) < 2:
        raise ValueError("LineString requires at least two coordinates")

    feature_id = str(features[0].get("id", "edge-0"))
    points = [_web_mercator(*coordinate[:2]) for coordinate in coordinates]
    z, x, y = _tile_key(*points[0])
    if any(_tile_key(*point) != (z, x, y) for point in points[1:]):
        raise ValueError("v0 compiler slice does not support lines spanning tiles")
    quantized = [_quantize(point, z, x, y) for point in points]
    edge_id = _stable_id(f"edge:{feature_id}")
    node_ids = (_stable_id(f"node:{feature_id}:0"), _stable_id(f"node:{feature_id}:1"))

    decoded = [
        (b"GEOM", z, x, y, _geometry_section(quantized)),
        (b"GRPH", 0, 0, 0, _graph_section(node_ids, edge_id)),
        (b"PROP", 0, 0, 0, _property_section(properties)),
        (b"STCH", 0, 0, 0, struct.pack("<3I", 0, 12, 12)),
    ]
    file_uuid = hashlib.sha256(b"".join(payload for *_, payload in decoded)).digest()[:16]
    stored = [(kind, z, x, y, payload, zstandard.ZstdCompressor(level=3).compress(payload)) for kind, z, x, y, payload in decoded]
    directory_bytes = len(stored) * _DIRECTORY.size
    offset = _HEADER.size + directory_bytes
    records = []
    for kind, z, x, y, payload, compressed in stored:
        records.append((kind, z, x, y, offset, compressed, payload))
        offset += len(compressed)

    header = _HEADER.pack(_MAGIC, 0, 1, 0, 0, 1, 0, 64, 64, len(records), 0, offset, file_uuid)
    directory = b"".join(
        _DIRECTORY.pack(
            kind, 1, z, x, y, section_offset, len(compressed), len(payload), _crc32c(payload), 1, 0, 0, b"\0" * 12
        )
        for kind, z, x, y, section_offset, compressed, payload in records
    )
    destination.write_bytes(header + directory + b"".join(compressed for *_, compressed, _payload in records))


def _geometry_section(points: list[tuple[int, int]]) -> bytes:
    path_header = struct.pack("<4I", 0, 0, len(points), 0)
    vertices = b"".join(struct.pack("<HH", *point) for point in points)
    mesh_vertices = b"".join(struct.pack("<HHI", point[0], point[1], 0) for point in points)
    header_size = 32
    paths_offset = header_size
    vertices_offset = paths_offset + len(path_header)
    mesh_vertices_offset = vertices_offset + len(vertices)
    mesh_indices_offset = mesh_vertices_offset + len(mesh_vertices)
    header = struct.pack("<8I", 1, len(points), len(points), 0, paths_offset, vertices_offset, mesh_vertices_offset, mesh_indices_offset)
    return header + path_header + vertices + mesh_vertices


def _graph_section(node_ids: tuple[int, int], edge_id: int) -> bytes:
    source_id, target_id = node_ids
    sorted_node_ids = sorted(node_ids)
    source_index = sorted_node_ids.index(source_id)
    target_index = sorted_node_ids.index(target_id)
    header_size = 36
    nodes_offset = header_size
    edges_offset = nodes_offset + 32
    csr_offset = edges_offset + 32
    adjacency_offset = csr_offset + 24
    refs_offset = adjacency_offset + 16
    header = struct.pack("<9I", 2, 1, 2, 1, nodes_offset, edges_offset, csr_offset, adjacency_offset, refs_offset)
    nodes = struct.pack("<QIIQII", sorted_node_ids[0], 0, 0, sorted_node_ids[1], 0, 0)
    edge = struct.pack("<QIIIIII", edge_id, source_index, target_index, 0, 0, 1, 0)
    csr = struct.pack("<3Q", 0, 1, 2)
    adjacency = b"".join(
        struct.pack("<Ib3x", 0, 1 if node_id == source_id else -1) for node_id in sorted_node_ids
    )
    geometry_ref = struct.pack("<IIb3x", 0, 0, 1)
    return header + nodes + edge + csr + adjacency + geometry_ref


def _property_section(properties: dict[str, object]) -> bytes:
    keys = sorted(properties)
    if any(not isinstance(key, str) for key in keys):
        raise ValueError("property keys must be strings")
    string_values = sorted({value for value in properties.values() if isinstance(value, str)})
    key_dictionary = _string_dictionary(keys)
    string_dictionary = _string_dictionary(string_values)
    enum_dictionary = struct.pack("<I", 0)
    header_size = 40
    key_offset = header_size
    string_offset = key_offset + len(key_dictionary)
    enum_offset = string_offset + len(string_dictionary)
    columns_offset = enum_offset + len(enum_dictionary)
    values_offset = columns_offset + len(keys) * 20
    columns: list[bytes] = []
    bodies: list[bytes] = []
    for key_id, key in enumerate(keys):
        value = properties[key]
        value_type, encoded = _property_value(value, string_values)
        presence_offset = values_offset + sum(len(body) for body in bodies)
        body = b"\x01" + encoded
        value_offset = presence_offset + 1
        columns.append(struct.pack("<IBBHIII", key_id, 2, value_type, 0, 1, presence_offset, value_offset))
        bodies.append(body)
    header = struct.pack(
        "<10I", len(keys), len(string_values), 0, 0, len(keys), key_offset, string_offset, enum_offset, columns_offset, columns_offset
    )
    return header + key_dictionary + string_dictionary + enum_dictionary + b"".join(columns) + b"".join(bodies)


def _string_dictionary(values: list[str]) -> bytes:
    return struct.pack("<I", len(values)) + b"".join(struct.pack("<I", len(value.encode())) + value.encode() for value in values)


def _property_value(value: object, string_values: list[str]) -> tuple[int, bytes]:
    if isinstance(value, bool):
        return 4, bytes([value])
    if isinstance(value, int):
        return 2, struct.pack("<q", value)
    if isinstance(value, float) and math.isfinite(value):
        return 1, struct.pack("<d", value)
    if isinstance(value, str):
        return 3, struct.pack("<I", string_values.index(value))
    raise ValueError("v0 properties support only finite numbers, strings, and booleans")


def _stable_id(value: str) -> int:
    return int.from_bytes(hashlib.sha256(value.encode()).digest()[:8], "little")


def _web_mercator(longitude: float, latitude: float) -> tuple[float, float]:
    latitude = max(min(float(latitude), 85.05112878), -85.05112878)
    x = float(longitude) * 20037508.342789244 / 180
    y = math.log(math.tan((90 + latitude) * math.pi / 360)) / (math.pi / 180)
    return x, y * 20037508.342789244 / 180


def _tile_key(x_m: float, y_m: float, z: int = 14) -> tuple[int, int, int]:
    world = 40075016.68557849
    tiles = 1 << z
    x = min(tiles - 1, max(0, int((x_m + world / 2) / world * tiles)))
    y = min(tiles - 1, max(0, int((world / 2 - y_m) / world * tiles)))
    return z, x, y


def _quantize(point: tuple[float, float], z: int, x: int, y: int) -> tuple[int, int]:
    world = 40075016.68557849
    width = world / (1 << z)
    min_x = -world / 2 + x * width
    max_y = world / 2 - y * width
    return (
        max(0, min(65535, round((point[0] - min_x) / width * 65535))),
        max(0, min(65535, round((max_y - point[1]) / width * 65535))),
    )


def _crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
    return (~crc) & 0xFFFFFFFF
