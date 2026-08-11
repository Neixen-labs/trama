# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""GeoJSON export for TRAMA v0 containers, per SPEC section 9."""

from __future__ import annotations

import json
import math
import struct
from pathlib import Path
from typing import Any

from trama_engine.compiler import parse_graph, read_sections

_WORLD = 40075016.68557849
_EXTENT = 65535


def export_geojson(source: Path, destination: Path) -> None:
    """Write `nodes.geojson` and `edges.geojson` into `destination` from a `.trama` file."""
    sections = read_sections(source.read_bytes())
    graph = next((payload for kind, _key, payload in sections if kind == b"GRPH"), None)
    properties = next((payload for kind, _key, payload in sections if kind == b"PROP"), None)
    if graph is None or properties is None:
        raise ValueError("container is missing a GRPH or PROP section")
    geometry = {
        index: (key, _parse_geometry(payload))
        for index, (kind, key, payload) in enumerate(sections)
        if kind == b"GEOM"
    }
    nodes, edges, refs = parse_graph(graph)
    rows = _parse_properties(properties)
    node_positions: dict[int, tuple[float, float]] = {}
    edge_features = []
    for edge_id, source_index, target_index, property_row, ref_start, ref_count, _flags in edges:
        coordinates = _edge_coordinates(refs[ref_start : ref_start + ref_count], geometry)
        node_positions[source_index] = coordinates[0]
        node_positions[target_index] = coordinates[-1]
        edge_features.append(
            _feature(
                {"type": "LineString", "coordinates": [list(point) for point in coordinates]},
                edge_id,
                rows[property_row] if property_row < len(rows) else {},
            )
        )

    node_features = [
        _feature({"type": "Point", "coordinates": list(node_positions[index])}, node_id, {})
        for index, (node_id, _property_row, _flags) in enumerate(nodes)
        if index in node_positions
    ]
    destination.mkdir(parents=True, exist_ok=True)
    _write(destination / "nodes.geojson", node_features)
    _write(destination / "edges.geojson", edge_features)


def _edge_coordinates(
    edge_refs: list[tuple[int, int, int]], geometry: dict[int, tuple[tuple[int, int, int], list[list[tuple[int, int]]]]]
) -> list[tuple[float, float]]:
    coordinates: list[tuple[float, float]] = []
    for directory_index, path_index, direction in edge_refs:
        if directory_index not in geometry:
            raise ValueError("edge references a section that is not GEOM")
        (z, x, y), paths = geometry[directory_index]
        if path_index >= len(paths):
            raise ValueError("edge references a missing path")
        piece = [_wgs84(*_dequantize(point, z, x, y)) for point in paths[path_index]]
        if direction < 0:
            piece.reverse()
        # Consecutive pieces meet at a tile boundary, so the shared vertex is already there.
        coordinates.extend(piece[1:] if coordinates else piece)
    if not coordinates:
        raise ValueError("edge has no geometry")
    return coordinates


def _feature(geometry: dict[str, Any], stable_id: int, properties: dict[str, Any]) -> dict[str, Any]:
    return {
        "type": "Feature",
        "geometry": geometry,
        "properties": {"_trama_id": str(stable_id), **properties},
    }


def _write(destination: Path, features: list[dict[str, Any]]) -> None:
    destination.write_text(json.dumps({"type": "FeatureCollection", "features": features}, indent=2) + "\n")


def _parse_geometry(payload: bytes) -> list[list[tuple[int, int]]]:
    path_count, _vertex_count = struct.unpack_from("<2I", payload)
    paths_offset, vertices_offset = struct.unpack_from("<2I", payload, 16)
    paths = []
    for index in range(path_count):
        _edge_index, first_vertex, vertex_count, _flags = struct.unpack_from("<4I", payload, paths_offset + index * 16)
        paths.append(
            [
                struct.unpack_from("<HH", payload, vertices_offset + (first_vertex + offset) * 4)
                for offset in range(vertex_count)
            ]
        )
    return paths


def _parse_properties(payload: bytes) -> list[dict[str, Any]]:
    _key_count, _string_count, _enum_count, _node_columns, edge_columns = struct.unpack_from("<5I", payload)
    key_offset, string_offset, _enum_offset, _node_columns_offset, columns_offset = struct.unpack_from("<5I", payload, 20)
    keys = _read_strings(payload, key_offset)
    strings = _read_strings(payload, string_offset)
    rows: list[dict[str, Any]] = []
    for column in range(edge_columns):
        key_id, _kind, value_type, _flags, entity_count, presence_offset, values_offset = struct.unpack_from(
            "<IBBHIII", payload, columns_offset + column * 20
        )
        while len(rows) < entity_count:
            rows.append({})
        present = [index for index in range(entity_count) if payload[presence_offset + index // 8] >> (index % 8) & 1]
        for dense, entity in enumerate(present):
            rows[entity][keys[key_id]] = _read_value(payload, values_offset, dense, value_type, strings)
    return rows


def _read_value(payload: bytes, values_offset: int, dense: int, value_type: int, strings: list[str]) -> Any:
    if value_type == 1:
        return struct.unpack_from("<d", payload, values_offset + dense * 8)[0]
    if value_type == 2:
        return struct.unpack_from("<q", payload, values_offset + dense * 8)[0]
    if value_type == 3:
        return strings[struct.unpack_from("<I", payload, values_offset + dense * 4)[0]]
    if value_type == 4:
        return bool(payload[values_offset + dense // 8] >> (dense % 8) & 1)
    raise ValueError(f"unsupported v0 property type {value_type}")


def _read_strings(payload: bytes, offset: int) -> list[str]:
    count = struct.unpack_from("<I", payload, offset)[0]
    values = []
    at = offset + 4
    for _index in range(count):
        length = struct.unpack_from("<I", payload, at)[0]
        values.append(payload[at + 4 : at + 4 + length].decode())
        at += 4 + length
    return values


def _dequantize(point: tuple[int, int], z: int, x: int, y: int) -> tuple[float, float]:
    width = _WORLD / (1 << z)
    return (
        -_WORLD / 2 + x * width + point[0] / _EXTENT * width,
        _WORLD / 2 - y * width - point[1] / _EXTENT * width,
    )


def _wgs84(x_m: float, y_m: float) -> tuple[float, float]:
    longitude = x_m * 180 / (_WORLD / 2)
    latitude = math.degrees(2 * math.atan(math.exp(y_m * math.pi / (_WORLD / 2))) - math.pi / 2)
    return longitude, latitude
