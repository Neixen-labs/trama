# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Write a network as a deterministic TRAMA container."""

from __future__ import annotations

import hashlib
import math
import struct
from pathlib import Path

import zstandard

from trama_engine import container
from trama_engine.container import BOOL, EDGE_KIND, F64, I64, NODE_KIND, STRING
from trama_engine.model import Network


def write(network: Network, destination: Path) -> None:
    """Serialize a network. Identical logical input always produces identical bytes."""
    node_index = {node.id: index for index, node in enumerate(network.nodes)}

    tiles: dict[tuple[int, int, int], list[int]] = {}
    for edge_index, edge in enumerate(network.edges):
        tiles.setdefault(_fit_tile(edge.points), []).append(edge_index)
    tile_keys = sorted(tiles)
    geometry_refs = {
        edge_index: (directory_index, path_index)
        for directory_index, tile in enumerate(tile_keys)
        for path_index, edge_index in enumerate(tiles[tile])
    }

    decoded = [
        (b"GEOM", *tile, _geometry_section(tile, [(index, network.edges[index].points) for index in tiles[tile]]))
        for tile in tile_keys
    ]
    decoded += [
        (b"GRPH", 0, 0, 0, _graph_section(network, node_index, geometry_refs)),
        (b"PROP", 0, 0, 0, _property_section(network)),
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


def _graph_section(network: Network, node_index: dict[int, int], geometry_refs: dict[int, tuple[int, int]]) -> bytes:
    adjacency_rows: list[list[tuple[int, int]]] = [[] for _ in network.nodes]
    for edge_index, edge in enumerate(network.edges):
        adjacency_rows[node_index[edge.source]].append((edge_index, 1))
        adjacency_rows[node_index[edge.target]].append((edge_index, -1))

    nodes = b"".join(struct.pack("<QII", node.id, index, 0) for index, node in enumerate(network.nodes))
    edge_records = b"".join(
        struct.pack(
            "<QIIIIII",
            edge.id,
            node_index[edge.source],
            node_index[edge.target],
            edge_index,
            edge_index,
            1,
            0,
        )
        for edge_index, edge in enumerate(network.edges)
    )
    csr = [0]
    for row in adjacency_rows:
        csr.append(csr[-1] + len(row))
    csr_offsets = b"".join(struct.pack("<Q", value) for value in csr)
    adjacency = b"".join(
        struct.pack("<Ib3x", edge_index, direction) for row in adjacency_rows for edge_index, direction in row
    )
    refs = b"".join(struct.pack("<IIb3x", *geometry_refs[index], 1) for index in range(len(network.edges)))

    header_size = 36
    nodes_offset = header_size
    edges_offset = nodes_offset + len(nodes)
    csr_offset = edges_offset + len(edge_records)
    adjacency_offset = csr_offset + len(csr_offsets)
    refs_offset = adjacency_offset + len(adjacency)
    header = struct.pack(
        "<9I",
        len(network.nodes),
        len(network.edges),
        csr[-1],
        len(network.edges),
        nodes_offset,
        edges_offset,
        csr_offset,
        adjacency_offset,
        refs_offset,
    )
    return header + nodes + edge_records + csr_offsets + adjacency + refs


def _property_section(network: Network) -> bytes:
    """Encode one typed column per entity kind and property key, ordered by kind then key."""
    rows_by_kind = [
        (NODE_KIND, [node.properties for node in network.nodes]),
        (EDGE_KIND, [edge.properties for edge in network.edges]),
    ]
    keys = sorted({key for _kind, rows in rows_by_kind for row in rows for key in row})
    strings = sorted({
        value
        for _kind, rows in rows_by_kind
        for row in rows
        for value in row.values()
        if isinstance(value, str)
    })
    string_ids = {value: index for index, value in enumerate(strings)}

    columns = []
    for kind, rows in rows_by_kind:
        for key_id, key in enumerate(keys):
            values = [row.get(key) for row in rows]
            present = [value for value in values if value is not None]
            if not present:
                continue
            bitmap = bytearray((len(values) + 7) // 8)
            for index, value in enumerate(values):
                if value is not None:
                    bitmap[index // 8] |= 1 << (index % 8)
            value_type = _column_type(key, present)
            columns.append(
                (
                    key_id,
                    kind,
                    value_type,
                    len(present) < len(values),
                    len(values),
                    bytes(bitmap),
                    _encode_values(value_type, present, string_ids),
                )
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
    for key_id, kind, value_type, nullable, entity_count, bitmap, values in columns:
        padding = -(offset + len(bitmap)) % 8
        records += container.COLUMN.pack(
            key_id, kind, value_type, 1 if nullable else 0, entity_count, offset, offset + len(bitmap) + padding
        )
        body += bitmap + b"\0" * padding + values
        offset += len(bitmap) + padding + len(values)

    header = struct.pack(
        "<10I",
        len(keys),
        len(strings),
        0,
        sum(1 for column in columns if column[1] == NODE_KIND),
        sum(1 for column in columns if column[1] == EDGE_KIND),
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
        raise ValueError(f"property '{key}' mixes value types across entities")
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
    """Empty STCH: an input file declares no state channels; solvers do."""
    return struct.pack("<3I", 0, 12, 16) + struct.pack("<I", 0)


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
