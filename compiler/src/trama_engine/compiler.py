# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Deterministic GeoJSON compiler for TRAMA v0."""

from __future__ import annotations

import hashlib
import json
import math
import struct
from pathlib import Path
from typing import NamedTuple

import zstandard

from trama_engine import container
from trama_engine.container import BOOL, F64, I64, STRING


class _Edge(NamedTuple):
    nodes: tuple[int, int]
    points: list[tuple[float, float]]
    properties: dict[str, object]


def compile_geojson(source: Path, destination: Path) -> None:
    """Compile GeoJSON LineString features into a deterministic TRAMA file."""
    edges = _read_edges(json.loads(source.read_text()))
    edge_ids = sorted(edges)
    node_ids = sorted({node_id for edge_id in edge_ids for node_id in edges[edge_id].nodes})
    node_index = {node_id: index for index, node_id in enumerate(node_ids)}

    tiles: dict[tuple[int, int, int], list[int]] = {}
    for edge_index, edge_id in enumerate(edge_ids):
        tiles.setdefault(_fit_tile(edges[edge_id].points), []).append(edge_index)
    tile_keys = sorted(tiles)
    geometry_refs = {
        edge_index: (directory_index, path_index)
        for directory_index, tile in enumerate(tile_keys)
        for path_index, edge_index in enumerate(tiles[tile])
    }

    decoded = [
        (b"GEOM", *tile, _geometry_section(tile, [(index, edges[edge_ids[index]].points) for index in tiles[tile]]))
        for tile in tile_keys
    ]
    decoded += [
        (b"GRPH", 0, 0, 0, _graph_section(node_ids, node_index, edge_ids, edges, geometry_refs)),
        (b"PROP", 0, 0, 0, _property_section(edge_ids, edges)),
        (b"STCH", 0, 0, 0, _state_channel_section()),
    ]

    file_uuid = hashlib.sha256(b"".join(payload for *_, payload in decoded)).digest()[:16]
    compressor = zstandard.ZstdCompressor(level=3)
    offset = container.HEADER.size + len(decoded) * container.DIRECTORY.size
    records = []
    for kind, z, x, y, payload in decoded:
        compressed = compressor.compress(payload)
        records.append((kind, z, x, y, offset, compressed, payload))
        offset += len(compressed)

    header = container.HEADER.pack(
        container.MAGIC,
        *container.FORMAT_VERSION,
        *container.MINIMUM_READER_VERSION,
        container.HEADER.size,
        container.HEADER.size,
        len(records),
        0,
        offset,
        file_uuid,
    )
    directory = b"".join(
        container.DIRECTORY.pack(
            kind,
            1,
            z,
            x,
            y,
            section_offset,
            len(compressed),
            len(payload),
            container.crc32c(payload),
            container.ZSTD,
            0,
            0,
            b"\0" * 12,
        )
        for kind, z, x, y, section_offset, compressed, payload in records
    )
    destination.write_bytes(header + directory + b"".join(compressed for *_, compressed, _payload in records))


def _read_edges(document: dict) -> dict[int, _Edge]:
    """Map every LineString feature to a stable edge id, its endpoint node ids, metres, and properties."""
    edges: dict[int, _Edge] = {}
    for feature in document.get("features") or []:
        geometry = feature.get("geometry") or {}
        if geometry.get("type") != "LineString":
            raise ValueError("v0 compiler accepts LineString features only")
        coordinates = geometry.get("coordinates") or []
        if len(coordinates) < 2:
            raise ValueError("LineString requires at least two coordinates")
        keys = [_node_key(coordinate) for coordinate in coordinates]
        properties = {key: value for key, value in (feature.get("properties") or {}).items() if value is not None}
        identity = feature.get("id")
        if "_trama_id" in properties:
            # An exported file carries its own identity back in; see SPEC section 8.
            exported = properties.pop("_trama_id")
            source_identity = f"_trama_id {exported!r}"
            edge_id = _exported_id(exported)
        else:
            # An id-less feature has no source identity, so derive one from its snapped geometry:
            # that keeps ids stable when features are reordered in the source document.
            source_identity = f"edge:{identity}" if identity is not None else "edge:geometry:" + ";".join(keys)
            edge_id = _stable_id(source_identity)
        if edge_id in edges:
            raise ValueError(f"duplicate edge identity: {source_identity}")
        edges[edge_id] = _Edge(
            (_stable_id(f"node:{keys[0]}"), _stable_id(f"node:{keys[-1]}")),
            [container.web_mercator(*coordinate[:2]) for coordinate in coordinates],
            properties,
        )
    if not edges:
        raise ValueError("GeoJSON contains no LineString features")
    return edges


def _geometry_section(tile: tuple[int, int, int], paths: list[tuple[int, list[tuple[float, float]]]]) -> bytes:
    header_size = 32
    path_records = b""
    vertices = b""
    first_vertex = 0
    for edge_index, points in paths:
        path_records += struct.pack("<4I", edge_index, first_vertex, len(points), 0)
        vertices += b"".join(struct.pack("<HH", *container.quantize(point, tile)) for point in points)
        first_vertex += len(points)
    paths_offset = header_size
    vertices_offset = paths_offset + len(path_records)
    end = vertices_offset + len(vertices)
    # ponytail: centerline paths only; the triangle mesh lands when the engine defines its vertex layout.
    header = struct.pack("<8I", len(paths), first_vertex, 0, 0, paths_offset, vertices_offset, end, end)
    return header + path_records + vertices


def _graph_section(
    node_ids: list[int],
    node_index: dict[int, int],
    edge_ids: list[int],
    edges: dict[int, _Edge],
    geometry_refs: dict[int, tuple[int, int]],
) -> bytes:
    adjacency_rows: list[list[tuple[int, int]]] = [[] for _ in node_ids]
    for edge_index, edge_id in enumerate(edge_ids):
        source_id, target_id = edges[edge_id].nodes
        adjacency_rows[node_index[source_id]].append((edge_index, 1))
        adjacency_rows[node_index[target_id]].append((edge_index, -1))

    nodes = b"".join(struct.pack("<QII", node_id, index, 0) for index, node_id in enumerate(node_ids))
    edge_records = b"".join(
        struct.pack(
            "<QIIIIII",
            edge_id,
            node_index[edges[edge_id].nodes[0]],
            node_index[edges[edge_id].nodes[1]],
            edge_index,
            edge_index,
            1,
            0,
        )
        for edge_index, edge_id in enumerate(edge_ids)
    )
    csr = [0]
    for row in adjacency_rows:
        csr.append(csr[-1] + len(row))
    csr_offsets = b"".join(struct.pack("<Q", value) for value in csr)
    adjacency = b"".join(
        struct.pack("<Ib3x", edge_index, direction) for row in adjacency_rows for edge_index, direction in row
    )
    refs = b"".join(struct.pack("<IIb3x", *geometry_refs[index], 1) for index in range(len(edge_ids)))

    header_size = 36
    nodes_offset = header_size
    edges_offset = nodes_offset + len(nodes)
    csr_offset = edges_offset + len(edge_records)
    adjacency_offset = csr_offset + len(csr_offsets)
    refs_offset = adjacency_offset + len(adjacency)
    header = struct.pack(
        "<9I",
        len(node_ids),
        len(edge_ids),
        csr[-1],
        len(edge_ids),
        nodes_offset,
        edges_offset,
        csr_offset,
        adjacency_offset,
        refs_offset,
    )
    return header + nodes + edge_records + csr_offsets + adjacency + refs


def _property_section(edge_ids: list[int], edges: dict[int, _Edge]) -> bytes:
    """Encode one typed column per property key, in `key_id` order.

    ponytail: edge columns only. GeoJSON LineStrings carry no node properties, so
    node columns arrive with the Point and CSV inputs that actually have them.
    """
    keys = sorted({key for edge_id in edge_ids for key in edges[edge_id].properties})
    strings = sorted({
        value
        for edge_id in edge_ids
        for value in edges[edge_id].properties.values()
        if isinstance(value, str)
    })
    string_ids = {value: index for index, value in enumerate(strings)}

    columns = []
    for key_id, key in enumerate(keys):
        values = [edges[edge_id].properties.get(key) for edge_id in edge_ids]
        present = [value for value in values if value is not None]
        value_type = _column_type(key, present)
        bitmap = bytearray((len(values) + 7) // 8)
        for index, value in enumerate(values):
            if value is not None:
                bitmap[index // 8] |= 1 << (index % 8)
        columns.append(
            (key_id, value_type, len(present) < len(values), bytes(bitmap), _encode_values(value_type, present, string_ids))
        )

    key_dictionary = container.pack_strings(keys)
    string_dictionary = container.pack_strings(strings)
    enum_dictionary = container.pack_strings([])
    header_size = 40
    key_dictionary_offset = header_size
    string_dictionary_offset = key_dictionary_offset + len(key_dictionary)
    enum_dictionary_offset = string_dictionary_offset + len(string_dictionary)
    columns_offset = enum_dictionary_offset + len(enum_dictionary)

    records = b""
    body = b""
    offset = columns_offset + len(columns) * container.COLUMN.size
    for key_id, value_type, nullable, bitmap, values in columns:
        padding = -(offset + len(bitmap)) % 8
        records += container.COLUMN.pack(
            key_id, container.EDGE_KIND, value_type, 1 if nullable else 0, len(edge_ids), offset, offset + len(bitmap) + padding
        )
        body += bitmap + b"\0" * padding + values
        offset += len(bitmap) + padding + len(values)

    header = struct.pack(
        "<10I",
        len(keys),
        len(strings),
        0,
        0,
        len(columns),
        key_dictionary_offset,
        string_dictionary_offset,
        enum_dictionary_offset,
        columns_offset,
        columns_offset,
    )
    return header + key_dictionary + string_dictionary + enum_dictionary + records + body


def _column_type(key: str, values: list[object]) -> int:
    """Pick one column type for a key, rejecting mixtures that would lose information."""
    kinds = {_value_kind(key, value) for value in values}
    if kinds <= {I64, F64} and F64 in kinds:
        return F64
    if len(kinds) != 1:
        raise ValueError(f"property '{key}' mixes value types across features")
    return kinds.pop()


def _value_kind(key: str, value: object) -> int:
    if isinstance(value, bool):
        return BOOL
    if isinstance(value, int):
        if not -(2**63) <= value < 2**63:
            raise ValueError(f"property '{key}' has an integer outside the i64 range")
        return I64
    if isinstance(value, float):
        if not math.isfinite(value):
            raise ValueError(f"property '{key}' has a non-finite number")
        return F64
    if isinstance(value, str):
        return STRING
    raise ValueError(f"property '{key}' has an unsupported value type: {type(value).__name__}")


def _encode_values(value_type: int, values: list[object], string_ids: dict[str, int]) -> bytes:
    if value_type == BOOL:
        packed = bytearray((len(values) + 7) // 8)
        for index, value in enumerate(values):
            if value:
                packed[index // 8] |= 1 << (index % 8)
        return bytes(packed)
    if value_type == STRING:
        return b"".join(struct.pack("<I", string_ids[value]) for value in values)
    if value_type == I64:
        return b"".join(struct.pack("<q", value) for value in values)
    return b"".join(struct.pack("<d", float(value)) for value in values)


def _state_channel_section() -> bytes:
    """Empty STCH: GeoJSON declares no state channels; solvers do."""
    return struct.pack("<3I", 0, 12, 16) + struct.pack("<I", 0)


def _exported_id(value: object) -> int:
    """Take back an ID this compiler wrote. A corrupt one is an error, never a silent renumber."""
    if not isinstance(value, str) or not value.isdigit() or not 0 <= int(value) < 2**64:
        raise ValueError(f"_trama_id must be a decimal u64 string, got {value!r}")
    return int(value)


def _stable_id(value: str) -> int:
    return int.from_bytes(hashlib.sha256(value.encode()).digest()[:8], "little")


def _node_key(coordinate: list[float]) -> str:
    """Snap an endpoint to a 1e-7 degree grid, roughly one centimetre.

    ponytail: exact key match, no spatial index. Add a tolerance flag when real
    sources arrive with endpoints that miss each other by more than a centimetre.
    """
    return ",".join(f"{round(float(value), 7) + 0.0:.7f}" for value in coordinate[:2])


def _fit_tile(points: list[tuple[float, float]]) -> tuple[int, int, int]:
    """Return the deepest tile that contains the whole line.

    ponytail: no tile clipping. Long lines drop to a coarse zoom instead of being
    split. Add real clipping when the engine needs level-of-detail selection.
    """
    for z in range(container.MAX_ZOOM, 0, -1):
        key = container.tile_key(*points[0], z)
        if all(container.tile_key(*point, z) == key for point in points[1:]):
            return key
    return 0, 0, 0

