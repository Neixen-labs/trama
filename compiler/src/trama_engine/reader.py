# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Decode a TRAMA file into plain Python structures.

ponytail: reads the whole file. HTTP range loading is the engine's problem, not
the exporters'; they need every section anyway.
"""

from __future__ import annotations

import struct
from pathlib import Path
from typing import NamedTuple

import zstandard

from trama_engine import container


class Section(NamedTuple):
    kind: bytes
    key: tuple[int, int, int]
    payload: bytes


class Edge(NamedTuple):
    id: int
    source: int
    target: int
    points: list[tuple[float, float]]
    properties: dict[str, object]


class Network(NamedTuple):
    node_ids: list[int]
    node_points: list[tuple[float, float]]
    edges: list[Edge]


def read_sections(path: Path) -> list[Section]:
    """Validate the container and return every decoded section, in directory order."""
    data = path.read_bytes()
    if len(data) < container.HEADER.size or data[:8] != container.MAGIC:
        raise ValueError("not a TRAMA file")
    _magic, *version, header_bytes, directory_offset, count, _flags, file_bytes, _uuid = container.HEADER.unpack_from(
        data, 0
    )
    if tuple(version[3:]) > container.FORMAT_VERSION:
        raise ValueError(f"file needs a reader for version {'.'.join(str(part) for part in version[3:])}")
    if header_bytes != container.HEADER.size:
        raise ValueError("unsupported header size")
    if file_bytes != len(data):
        raise ValueError("file length disagrees with the header")

    sections = []
    for index in range(count):
        record = directory_offset + index * container.DIRECTORY.size
        if record + container.DIRECTORY.size > len(data):
            raise ValueError("section directory runs past the end of the file")
        kind, _record_flags, z, x, y, offset, stored, decoded_bytes, checksum, codec, *_ = container.DIRECTORY.unpack_from(
            data, record
        )
        if codec != container.ZSTD:
            raise ValueError(f"unsupported section codec: {codec}")
        if offset + stored > len(data):
            raise ValueError(f"section {kind.decode()} runs past the end of the file")
        try:
            payload = zstandard.ZstdDecompressor().decompress(data[offset : offset + stored])
        except zstandard.ZstdError as error:
            raise ValueError(f"section {kind.decode()} is not a readable zstd frame") from error
        if len(payload) != decoded_bytes:
            raise ValueError(f"section {kind.decode()} decoded to an unexpected length")
        if container.crc32c(payload) != checksum:
            raise ValueError(f"section {kind.decode()} failed its checksum")
        sections.append(Section(kind, (z, x, y), payload))
    return sections


def read_network(path: Path) -> Network:
    """Rebuild nodes, edges, geometry, and properties from a TRAMA file."""
    sections = read_sections(path)
    graph = _single(sections, b"GRPH")
    properties = _read_properties(_single(sections, b"PROP"))

    node_count, edge_count, _adjacency_count, _ref_count = struct.unpack_from("<4I", graph, 0)
    nodes_offset, edges_offset, _csr_offset, _adjacency_offset, refs_offset = struct.unpack_from("<5I", graph, 16)
    node_ids = [struct.unpack_from("<Q", graph, nodes_offset + index * 16)[0] for index in range(node_count)]

    edges = []
    node_points: list[tuple[float, float] | None] = [None] * node_count
    for index in range(edge_count):
        edge_id, source, target, _row, ref_start, ref_count, _flags = struct.unpack_from(
            "<QIIIIII", graph, edges_offset + index * 32
        )
        if source >= node_count or target >= node_count:
            raise ValueError("edge references a node outside the node array")
        points: list[tuple[float, float]] = []
        for ref in range(ref_start, ref_start + ref_count):
            directory_index, path_index, direction = struct.unpack_from("<IIb", graph, refs_offset + ref * 12)
            segment = _read_path(sections, directory_index, path_index, index)
            points += segment if direction >= 0 else segment[::-1]
        edges.append(Edge(edge_id, source, target, points, properties.get(index, {})))
        node_points[source] = node_points[source] or points[0]
        node_points[target] = node_points[target] or points[-1]

    missing = [node_ids[index] for index, point in enumerate(node_points) if point is None]
    if missing:
        raise ValueError(f"{len(missing)} nodes have no incident edge geometry")
    return Network(node_ids, [point for point in node_points if point is not None], edges)


def _single(sections: list[Section], kind: bytes) -> bytes:
    matches = [section.payload for section in sections if section.kind == kind]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {kind.decode()} section, found {len(matches)}")
    return matches[0]


def _read_path(sections: list[Section], directory_index: int, path_index: int, edge_index: int) -> list[tuple[float, float]]:
    if directory_index >= len(sections) or sections[directory_index].kind != b"GEOM":
        raise ValueError("geometry reference does not point at a GEOM section")
    tile = sections[directory_index].key
    payload = sections[directory_index].payload
    path_count, _vertex_count, _mesh_vertices, _mesh_indices = struct.unpack_from("<4I", payload, 0)
    paths_offset, vertices_offset = struct.unpack_from("<2I", payload, 16)
    if path_index >= path_count:
        raise ValueError("geometry reference points past the end of a tile")
    path_edge, first_vertex, vertex_count, _flags = struct.unpack_from("<4I", payload, paths_offset + path_index * 16)
    if path_edge != edge_index:
        raise ValueError("geometry path does not belong to the edge referencing it")
    return [
        container.unquantize(*struct.unpack_from("<HH", payload, vertices_offset + (first_vertex + vertex) * 4), tile)
        for vertex in range(vertex_count)
    ]


def _read_properties(payload: bytes) -> dict[int, dict[str, object]]:
    """Decode edge property columns into {entity index: {key: value}}."""
    _key_count, _string_count, _enum_count, node_columns, edge_columns = struct.unpack_from("<5I", payload, 0)
    key_offset, string_offset, _enum_offset, _node_offset, columns_offset = struct.unpack_from("<5I", payload, 20)
    keys = container.unpack_strings(payload, key_offset)
    strings = container.unpack_strings(payload, string_offset)

    rows: dict[int, dict[str, object]] = {}
    for index in range(node_columns + edge_columns):
        key_id, kind, value_type, _flags, entity_count, bitmap_offset, values_offset = container.COLUMN.unpack_from(
            payload, columns_offset + index * container.COLUMN.size
        )
        if kind != container.EDGE_KIND:
            continue
        position = 0
        for entity in range(entity_count):
            if not payload[bitmap_offset + entity // 8] >> (entity % 8) & 1:
                continue
            rows.setdefault(entity, {})[keys[key_id]] = _read_value(
                payload, values_offset, position, value_type, strings
            )
            position += 1
    return rows


def _read_value(payload: bytes, values_offset: int, position: int, value_type: int, strings: list[str]) -> object:
    if value_type == container.F64:
        return struct.unpack_from("<d", payload, values_offset + position * 8)[0]
    if value_type == container.I64:
        return struct.unpack_from("<q", payload, values_offset + position * 8)[0]
    if value_type == container.STRING:
        return strings[struct.unpack_from("<I", payload, values_offset + position * 4)[0]]
    if value_type == container.BOOL:
        return bool(payload[values_offset + position // 8] >> (position % 8) & 1)
    raise ValueError(f"unsupported property value type: {value_type}")
