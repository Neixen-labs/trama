# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Minimal deterministic GeoJSON compiler for TRAMA v0."""

from __future__ import annotations

import hashlib
import itertools
import json
import math
import struct
from pathlib import Path
from typing import Any

import zstandard

_MAGIC = b"TRAMA\0\0\0"
_HEADER = struct.Struct("<8s3H3HIQIIQ16s")
_DIRECTORY = struct.Struct("<4sIIIIQQQIHBB12s")


def compile_geojson(source: Path, destination: Path) -> None:
    """Compile a GeoJSON LineString collection into a TRAMA file, one GEOM record per tile."""
    document = json.loads(source.read_text())
    features = document.get("features", [])
    if not features or any(feature.get("geometry", {}).get("type") != "LineString" for feature in features):
        raise ValueError("v0 compiler slice requires one LineString feature")
    properties = [feature.get("properties") or {} for feature in features]
    if any(not isinstance(value, dict) for value in properties):
        raise TypeError("GeoJSON properties must be an object")

    lines = []
    for index, feature in enumerate(features):
        coordinates = feature["geometry"].get("coordinates", [])
        if len(coordinates) < 2:
            raise ValueError("LineString requires at least two coordinates")
        points = [_web_mercator(*coordinate[:2]) for coordinate in coordinates]
        lines.append((str(feature.get("id", f"edge-{index}")), coordinates, points))
    node_ids = {
        tuple(coordinates[endpoint][:2]): _stable_id(f"node:{coordinates[endpoint][0]!r},{coordinates[endpoint][1]!r}")
        for _feature_id, coordinates, _points in lines
        for endpoint in (0, -1)
    }
    ordered = sorted(
        (
            (
                (
                    _stable_id(f"edge:{feature_id}"),
                    node_ids[tuple(coordinates[0][:2])],
                    node_ids[tuple(coordinates[-1][:2])],
                ),
                _split_by_tile(points),
                row,
            )
            for (feature_id, coordinates, points), row in zip(lines, properties)
        ),
        key=lambda record: record[0][0],
    )
    edges = [edge for edge, _pieces, _row in ordered]
    edge_properties = [row for _edge, _pieces, row in ordered]
    if len({edge_id for edge_id, *_rest in edges}) != len(edges):
        raise ValueError("GeoJSON feature IDs must be unique")

    tiles = sorted({tile for _edge, pieces, _row in ordered for tile, _points in pieces})
    tile_indexes = {tile: index for index, tile in enumerate(tiles)}
    tile_paths: dict[tuple[int, int, int], list[tuple[int, list[tuple[int, int]]]]] = {tile: [] for tile in tiles}
    geometry_refs: list[list[tuple[int, int]]] = []
    for edge_index, (_edge, pieces, _row) in enumerate(ordered):
        refs = []
        for tile, points in pieces:
            paths = tile_paths[tile]
            refs.append((tile_indexes[tile], len(paths)))
            paths.append((edge_index, [_quantize(point, *tile) for point in points]))
        geometry_refs.append(refs)

    decoded = [(b"GEOM", *tile, _geometry_section(tile_paths[tile])) for tile in tiles] + [
        (b"GRPH", 0, 0, 0, _graph_section(edges, geometry_refs)),
        (b"PROP", 0, 0, 0, _property_section(edge_properties)),
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


def validate_container(source: Path) -> None:
    """Validate v0 container framing, compression, decoded lengths, and checksums."""
    data = source.read_bytes()
    if len(data) < _HEADER.size:
        raise ValueError("container is shorter than its header")
    magic, *_versions, header_bytes, directory_offset, section_count, _flags, file_bytes, _uuid = _HEADER.unpack_from(data)
    if magic != _MAGIC or header_bytes != _HEADER.size or directory_offset != _HEADER.size or file_bytes != len(data):
        raise ValueError("invalid container header")
    directory_end = directory_offset + section_count * _DIRECTORY.size
    if directory_end > len(data):
        raise ValueError("container directory exceeds file size")
    for index in range(section_count):
        record = _DIRECTORY.unpack_from(data, directory_offset + index * _DIRECTORY.size)
        _kind, _flags, _z, _x, _y, offset, stored_bytes, decoded_bytes, checksum, codec, _alignment, _reserved, _padding = record
        if codec != 1 or offset < directory_end or offset + stored_bytes > len(data):
            raise ValueError("invalid section record")
        try:
            decoded = zstandard.ZstdDecompressor().decompress(data[offset : offset + stored_bytes])
        except zstandard.ZstdError as error:
            raise ValueError("invalid zstd section") from error
        if len(decoded) != decoded_bytes or _crc32c(decoded) != checksum:
            raise ValueError("invalid section integrity")


def _split_by_tile(points: list[tuple[float, float]]) -> list[tuple[tuple[int, int, int], list[tuple[float, float]]]]:
    """Cut a projected polyline at tile boundaries, in traversal order."""
    pieces: list[tuple[tuple[int, int, int], list[tuple[float, float]]]] = []
    for start_point, end_point in itertools.pairwise(points):
        cuts = [0.0, *_boundary_crossings(start_point, end_point), 1.0]
        for span_start, span_end in itertools.pairwise(cuts):
            tile = _tile_key(*_interpolate(start_point, end_point, (span_start + span_end) / 2))
            piece_start = start_point if span_start == 0.0 else _interpolate(start_point, end_point, span_start)
            piece_end = end_point if span_end == 1.0 else _interpolate(start_point, end_point, span_end)
            if pieces and pieces[-1][0] == tile:
                pieces[-1][1].append(piece_end)
            else:
                pieces.append((tile, [piece_start, piece_end]))
    return pieces


def _boundary_crossings(start_point: tuple[float, float], end_point: tuple[float, float], z: int = 14) -> list[float]:
    """Fractions of the segment at which it crosses a tile edge, ascending."""
    width = 40075016.68557849 / (1 << z)
    crossings = []
    for axis in (0, 1):
        span = end_point[axis] - start_point[axis]
        if span == 0:
            continue
        low, high = sorted((start_point[axis], end_point[axis]))
        for step in range(math.floor(low / width) + 1, math.ceil(high / width)):
            fraction = (step * width - start_point[axis]) / span
            if 0.0 < fraction < 1.0:
                crossings.append(fraction)
    return sorted(crossings)


def _interpolate(start_point: tuple[float, float], end_point: tuple[float, float], fraction: float) -> tuple[float, float]:
    return (
        start_point[0] + (end_point[0] - start_point[0]) * fraction,
        start_point[1] + (end_point[1] - start_point[1]) * fraction,
    )


def _geometry_section(paths: list[tuple[int, list[tuple[int, int]]]]) -> bytes:
    first_vertices = [sum(len(path) for _edge_index, path in paths[:index]) for index in range(len(paths))]
    path_headers = b"".join(
        struct.pack("<4I", edge_index, first_vertex, len(path), 0)
        for (edge_index, path), first_vertex in zip(paths, first_vertices)
    )
    vertices = b"".join(struct.pack("<HH", *point) for _edge_index, path in paths for point in path)
    mesh_vertices = b"".join(
        struct.pack("<HHI", point[0], point[1], edge_index) for edge_index, path in paths for point in path
    )
    header_size = 32
    paths_offset = header_size
    vertices_offset = paths_offset + len(path_headers)
    mesh_vertices_offset = vertices_offset + len(vertices)
    mesh_indices_offset = mesh_vertices_offset + len(mesh_vertices)
    vertex_count = sum(len(path) for _edge_index, path in paths)
    header = struct.pack("<8I", len(paths), vertex_count, vertex_count, 0, paths_offset, vertices_offset, mesh_vertices_offset, mesh_indices_offset)
    return header + path_headers + vertices + mesh_vertices


def _graph_section(edges: list[tuple[int, int, int]], geometry_refs: list[list[tuple[int, int]]]) -> bytes:
    node_ids = sorted({node_id for _edge_id, source_id, target_id in edges for node_id in (source_id, target_id)})
    node_indices = {node_id: index for index, node_id in enumerate(node_ids)}
    adjacency: list[list[tuple[int, int]]] = [[] for _node_id in node_ids]
    for edge_index, (_edge_id, source_id, target_id) in enumerate(edges):
        adjacency[node_indices[source_id]].append((edge_index, 1))
        adjacency[node_indices[target_id]].append((edge_index, -1))
    ref_starts = [sum(len(refs) for refs in geometry_refs[:index]) for index in range(len(geometry_refs))]
    ref_count = sum(len(refs) for refs in geometry_refs)
    header_size = 36
    nodes_offset = header_size
    edges_offset = nodes_offset + len(node_ids) * 16
    csr_offset = edges_offset + len(edges) * 32
    adjacency_offset = csr_offset + (len(node_ids) + 1) * 8
    adjacency_count = sum(len(entries) for entries in adjacency)
    refs_offset = adjacency_offset + adjacency_count * 8
    header = struct.pack("<9I", len(node_ids), len(edges), adjacency_count, ref_count, nodes_offset, edges_offset, csr_offset, adjacency_offset, refs_offset)
    nodes = b"".join(struct.pack("<QII", node_id, 0, 0) for node_id in node_ids)
    edge_records = b"".join(
        struct.pack(
            "<QIIIIII", edge_id, node_indices[source_id], node_indices[target_id], edge_index, ref_start, len(refs), 0
        )
        for edge_index, ((edge_id, source_id, target_id), ref_start, refs) in enumerate(zip(edges, ref_starts, geometry_refs))
    )
    offsets = [0]
    for entries in adjacency:
        offsets.append(offsets[-1] + len(entries))
    csr = struct.pack(f"<{len(offsets)}Q", *offsets)
    adjacency_records = b"".join(struct.pack("<Ib3x", edge_index, direction) for entries in adjacency for edge_index, direction in entries)
    ref_records = b"".join(
        struct.pack("<IIb3x", directory_index, path_index, 1) for refs in geometry_refs for directory_index, path_index in refs
    )
    return header + nodes + edge_records + csr + adjacency_records + ref_records


def _property_section(rows: list[dict[str, object]]) -> bytes:
    keys = sorted({key for row in rows for key, value in row.items() if value is not None})
    string_values = sorted(
        {value for row in rows for value in row.values() if isinstance(value, str)}
    )
    key_dictionary = _string_dictionary(keys)
    string_dictionary = _string_dictionary(string_values)
    enum_dictionary = struct.pack("<I", 0)
    header_size = 40
    key_offset = header_size
    string_offset = key_offset + len(key_dictionary)
    enum_offset = string_offset + len(string_dictionary)
    columns_offset = enum_offset + len(enum_dictionary)
    values_offset = columns_offset + len(keys) * 20
    bitmap_bytes = (len(rows) + 7) // 8
    columns: list[bytes] = []
    bodies: list[bytes] = []
    for key_id, key in enumerate(keys):
        present = [index for index, row in enumerate(rows) if row.get(key) is not None]
        values = [rows[index][key] for index in present]
        value_type = _column_type(key, values)
        presence_offset = values_offset + sum(len(body) for body in bodies)
        columns.append(
            struct.pack(
                "<IBBHIII", key_id, 2, value_type, 1, len(rows), presence_offset, presence_offset + bitmap_bytes
            )
        )
        bodies.append(_packed_bits(present, len(rows)) + _column_values(value_type, values, string_values))
    header = struct.pack(
        "<10I", len(keys), len(string_values), 0, 0, len(keys), key_offset, string_offset, enum_offset, columns_offset, columns_offset
    )
    return header + key_dictionary + string_dictionary + enum_dictionary + b"".join(columns) + b"".join(bodies)


def _packed_bits(set_indexes: list[int], count: int) -> bytes:
    bits = bytearray((count + 7) // 8)
    for index in set_indexes:
        bits[index // 8] |= 1 << (index % 8)
    return bytes(bits)


def _column_type(key: str, values: list[Any]) -> int:
    types = {_value_type(value) for value in values}
    if types == {1, 2}:
        return 1
    if len(types) != 1:
        raise ValueError(f"property {key!r} mixes conflicting types across features")
    return types.pop()


def _column_values(value_type: int, values: list[Any], string_values: list[str]) -> bytes:
    if value_type == 4:
        return _packed_bits([index for index, value in enumerate(values) if value], len(values))
    if value_type == 3:
        return b"".join(struct.pack("<I", string_values.index(value)) for value in values)
    if value_type == 2:
        return b"".join(struct.pack("<q", value) for value in values)
    return b"".join(struct.pack("<d", value) for value in values)


def _string_dictionary(values: list[str]) -> bytes:
    return struct.pack("<I", len(values)) + b"".join(struct.pack("<I", len(value.encode())) + value.encode() for value in values)


def _value_type(value: object) -> int:
    if isinstance(value, bool):
        return 4
    if isinstance(value, int):
        return 2
    if isinstance(value, float) and math.isfinite(value):
        return 1
    if isinstance(value, str):
        return 3
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
