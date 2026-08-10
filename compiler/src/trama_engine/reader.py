# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Decode a TRAMA file into the network model.

ponytail: reads the whole file. HTTP range loading is the engine's problem, not
the exporters'; they need every section anyway.
"""

from __future__ import annotations

import struct
from pathlib import Path
from typing import NamedTuple

import zstandard

from trama_engine import container
from trama_engine.model import Edge, Network, Node


class Section(NamedTuple):
    kind: bytes
    key: tuple[int, int, int]
    payload: bytes


def read_sections(path: Path) -> list[Section]:
    """Validate the container and return every decoded section, in directory order."""
    data = path.read_bytes()
    if len(data) < container.HEADER.size or data[:8] != container.MAGIC:
        raise ValueError("not a TRAMA file")
    _magic, *versions, header_bytes, directory_offset, count, _flags, file_bytes, _uuid = container.HEADER.unpack_from(
        data, 0
    )
    minimum_reader = tuple(versions[3:])
    if minimum_reader > container.FORMAT_VERSION:
        raise ValueError(f"file needs a reader for version {'.'.join(str(part) for part in minimum_reader)}")
    if header_bytes != container.HEADER.size:
        raise ValueError("unsupported header size")
    if file_bytes != len(data):
        raise ValueError("file length disagrees with the header")

    sections = []
    for index in range(count):
        record = directory_offset + index * container.DIRECTORY.size
        if record + container.DIRECTORY.size > len(data):
            raise ValueError("section directory runs past the end of the file")
        kind, _record_flags, z, x, y, offset, stored, decoded_bytes, checksum, codec, *_ = (
            container.DIRECTORY.unpack_from(data, record)
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
    """Rebuild nodes, edges, geometry, and typed properties from a TRAMA file."""
    sections = read_sections(path)
    graph = _single(sections, b"GRPH")
    node_properties, edge_properties = _read_properties(_single(sections, b"PROP"))

    node_count, edge_count, _adjacency_count, _ref_count = struct.unpack_from("<4I", graph, 0)
    nodes_offset, edges_offset, _csr_offset, _adjacency_offset, refs_offset = struct.unpack_from("<5I", graph, 16)
    node_ids = [struct.unpack_from("<Q", graph, nodes_offset + index * 16)[0] for index in range(node_count)]

    edges = []
    points_by_node: dict[int, tuple[float, float]] = {}
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
        edges.append(Edge(edge_id, node_ids[source], node_ids[target], points, edge_properties.get(index, {})))
        points_by_node.setdefault(node_ids[source], points[0])
        points_by_node.setdefault(node_ids[target], points[-1])

    missing = [node_id for node_id in node_ids if node_id not in points_by_node]
    if missing:
        raise ValueError(f"{len(missing)} nodes have no incident edge geometry")
    nodes = [
        Node(node_id, points_by_node[node_id], node_properties.get(index, {}))
        for index, node_id in enumerate(node_ids)
    ]
    return Network(nodes, edges)


def _single(sections: list[Section], kind: bytes) -> bytes:
    matches = [section.payload for section in sections if section.kind == kind]
    if len(matches) != 1:
        raise ValueError(f"expected exactly one {kind.decode()} section, found {len(matches)}")
    return matches[0]


def _read_path(
    sections: list[Section], directory_index: int, path_index: int, edge_index: int
) -> list[tuple[float, float]]:
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


def _read_properties(payload: bytes) -> tuple[dict[int, dict[str, object]], dict[int, dict[str, object]]]:
    """Decode property columns into ({node index: row}, {edge index: row})."""
    _key_count, _string_count, _enum_count, node_columns, edge_columns = struct.unpack_from("<5I", payload, 0)
    key_offset, string_offset, _enum_offset, _node_offset, columns_offset = struct.unpack_from("<5I", payload, 20)
    keys = container.unpack_strings(payload, key_offset)
    strings = container.unpack_strings(payload, string_offset)

    rows: dict[int, dict[int, dict[str, object]]] = {container.NODE_KIND: {}, container.EDGE_KIND: {}}
    for index in range(node_columns + edge_columns):
        key_id, kind, value_type, _flags, entity_count, bitmap_offset, values_offset = container.COLUMN.unpack_from(
            payload, columns_offset + index * container.COLUMN.size
        )
        if kind not in rows:
            raise ValueError(f"unsupported property entity kind: {kind}")
        position = 0
        for entity in range(entity_count):
            if not payload[bitmap_offset + entity // 8] >> (entity % 8) & 1:
                continue
            rows[kind].setdefault(entity, {})[keys[key_id]] = _read_value(
                payload, values_offset, position, value_type, strings
            )
            position += 1
    return rows[container.NODE_KIND], rows[container.EDGE_KIND]


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
