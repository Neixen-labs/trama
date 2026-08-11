# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Minimal deterministic GeoJSON compiler for TRAMA v0."""

from __future__ import annotations

import hashlib
import itertools
import json
import math
import re
import struct
from pathlib import Path
from typing import Any, NamedTuple

import zstandard

_MAGIC = b"TRAMA\0\0\0"
_HEADER = struct.Struct("<8s3H3HIQIIQ16s")
_DIRECTORY = struct.Struct("<4sIIIIQQQIHBB12s")
_ID_KEY = "_trama_id"
_EXTENT = 65535
_OWNER = re.compile(r"[a-z0-9-]+")
_EXTRA_HEADER = struct.Struct("<7I")


class Extra(NamedTuple):
    """An opaque record the core carries for someone else, per SPEC 7.

    The compiler never looks inside `payload`. Its whole job here is to make sure the record
    is additive: written optional, keyed at no tile, and removable without changing the rest.
    """

    owner: str
    media_type: str
    payload: bytes
# Deterministic and not format-significant (SPEC 8). 19 costs ~1.7 s on a 50k-edge network
# and saves 8% of the file, which is paid back on every download.
_COMPRESSION_LEVEL = 19


def compile_geojson(
    source: Path,
    destination: Path,
    channels: list[dict[str, Any]] | None = None,
    extras: list[Extra] | None = None,
) -> None:
    """Compile GeoJSON into a TRAMA file, one GEOM record per tile.

    `source` is a FeatureCollection, or a directory holding the `edges.geojson` and
    `nodes.geojson` an export wrote. A feature carrying `_trama_id` keeps that identity.
    `extras` are opaque records to carry along for their owners, per SPEC 7.
    """
    features = [feature for path in _sources(source) for feature in json.loads(path.read_text()).get("features", [])]
    compile_features(features, destination, channels, extras)


def compile_features(
    features: list[dict[str, Any]],
    destination: Path,
    channels: list[dict[str, Any]] | None = None,
    extras: list[Extra] | None = None,
) -> None:
    """Compile GeoJSON features already in memory, whoever produced them.

    This is the entry point an importer reaches: the compiler knows GeoJSON geometry and
    typed properties, and stays unable to name the format the features came from.
    """
    lines = [feature for feature in features if feature.get("geometry", {}).get("type") == "LineString"]
    points = [feature for feature in features if feature.get("geometry", {}).get("type") == "Point"]
    if not lines or len(lines) + len(points) != len(features):
        raise ValueError("v0 compiler slice requires one LineString feature")
    properties = [feature.get("properties") or {} for feature in lines]
    if any(not isinstance(value, dict) for value in properties):
        raise TypeError("GeoJSON properties must be an object")

    line_records = []
    for index, feature in enumerate(lines):
        coordinates = feature["geometry"].get("coordinates", [])
        if len(coordinates) < 2:
            raise ValueError("LineString requires at least two coordinates")
        line_records.append((str(feature.get("id", f"edge-{index}")), [_web_mercator(*coordinate[:2]) for coordinate in coordinates]))
    # A Point only names a node; an exported nodes.geojson is how node identity survives a round trip.
    node_ids = {
        cell: _stable_id(f"node:{cell[0]},{cell[1]}")
        for _feature_id, projected in line_records
        for cell in (_node_cell(projected[0]), _node_cell(projected[-1]))
    }
    node_properties: dict[tuple[int, int], dict[str, Any]] = {}
    for feature in points:
        coordinate = feature["geometry"].get("coordinates", [])[:2]
        cell = _node_cell(_web_mercator(*coordinate))
        declared = _declared_id(feature)
        if declared is not None:
            node_ids[cell] = declared
        row = feature.get("properties") or {}
        if not isinstance(row, dict):
            raise TypeError("GeoJSON properties must be an object")
        node_properties[cell] = {key: value for key, value in row.items() if key != _ID_KEY}
    ordered = sorted(
        (
            (
                (
                    _declared(feature, f"edge:{feature_id}"),
                    node_ids[_node_cell(projected[0])],
                    node_ids[_node_cell(projected[-1])],
                ),
                _split_by_tile(projected),
                {key: value for key, value in row.items() if key != _ID_KEY},
            )
            for feature, (feature_id, projected), row in zip(lines, line_records, properties)
        ),
        key=lambda record: record[0][0],
    )
    edges = [edge for edge, _pieces, _row in ordered]
    edge_properties = [row for _edge, _pieces, row in ordered]
    if len({edge_id for edge_id, *_rest in edges}) != len(edges):
        raise ValueError("GeoJSON feature IDs must be unique")
    # Node rows follow the node array, which _graph_section sorts by stable ID. A Point whose
    # position no edge touches never becomes a node, so its properties have nowhere to go.
    rows_by_id = {node_ids[cell]: row for cell, row in node_properties.items()}
    node_rows = [rows_by_id.get(node_id, {}) for node_id in _node_order(edges)]

    tiles = sorted({tile for _edge, pieces, _row in ordered for tile, _points in pieces})
    tile_indexes = {tile: index for index, tile in enumerate(tiles)}
    tile_paths: dict[tuple[int, int, int], list[tuple[int, list[tuple[int, int]]]]] = {tile: [] for tile in tiles}
    geometry_refs: list[list[tuple[int, int]]] = []
    for edge_index, (_edge, pieces, _row) in enumerate(ordered):
        refs = []
        for tile, piece in pieces:
            paths = tile_paths[tile]
            refs.append((tile_indexes[tile], len(paths)))
            paths.append((edge_index, [_quantize(point, *tile) for point in piece]))
        geometry_refs.append(refs)

    # Required sections carry record_flags bit 0; XTRA never does, which is what lets a reader
    # that has never heard of its owner skip it instead of rejecting the file (SPEC 7).
    decoded = [(b"GEOM", 1, *tile, _geometry_section(tile_paths[tile])) for tile in tiles] + [
        (b"GRPH", 1, 0, 0, 0, _graph_section(edges, geometry_refs)),
        (b"PROP", 1, 0, 0, 0, _property_section(node_rows, edge_properties)),
        # SPEC 6: strings_offset must address a u32 count, so an empty table still needs those 4 bytes.
        (b"STCH", 1, 0, 0, 0, _state_channel_section(channels or [])),
        *((b"XTRA", 0, 0, 0, 0, payload) for payload in _extra_sections(extras or [])),
    ]
    file_uuid = hashlib.sha256(b"".join(payload for *_, payload in decoded)).digest()[:16]
    stored = [(kind, flags, z, x, y, payload, zstandard.ZstdCompressor(level=_COMPRESSION_LEVEL).compress(payload)) for kind, flags, z, x, y, payload in decoded]
    directory_bytes = len(stored) * _DIRECTORY.size
    offset = _HEADER.size + directory_bytes
    records = []
    for kind, flags, z, x, y, payload, compressed in stored:
        records.append((kind, flags, z, x, y, offset, compressed, payload))
        offset += len(compressed)

    header = _HEADER.pack(_MAGIC, 0, 1, 0, 0, 1, 0, 64, 64, len(records), 0, offset, file_uuid)
    directory = b"".join(
        _DIRECTORY.pack(
            kind, flags, z, x, y, section_offset, len(compressed), len(payload), _crc32c(payload), 1, 0, 0, b"\0" * 12
        )
        for kind, flags, z, x, y, section_offset, compressed, payload in records
    )
    destination.write_bytes(header + directory + b"".join(compressed for *_, compressed, _payload in records))


def validate_container(source: Path) -> None:
    """Validate v0 container framing, compression, decoded lengths, and checksums."""
    read_sections(source.read_bytes())


def read_sections(data: bytes) -> list[tuple[bytes, tuple[int, int, int], bytes]]:
    """Verify a v0 container and return every section as (kind, tile key, decoded payload)."""
    if len(data) < _HEADER.size:
        raise ValueError("container is shorter than its header")
    magic, *_versions, header_bytes, directory_offset, section_count, _flags, file_bytes, _uuid = _HEADER.unpack_from(data)
    if magic != _MAGIC or header_bytes != _HEADER.size or directory_offset != _HEADER.size or file_bytes != len(data):
        raise ValueError("invalid container header")
    directory_end = directory_offset + section_count * _DIRECTORY.size
    if directory_end > len(data):
        raise ValueError("container directory exceeds file size")
    sections = []
    owners: set[tuple[str, str]] = set()
    for index in range(section_count):
        record = _DIRECTORY.unpack_from(data, directory_offset + index * _DIRECTORY.size)
        kind, flags, z, x, y, offset, stored_bytes, decoded_bytes, checksum, codec, _alignment, _reserved, _padding = record
        if codec != 1 or offset < directory_end or offset + stored_bytes > len(data):
            raise ValueError("invalid section record")
        try:
            decoded = zstandard.ZstdDecompressor().decompress(data[offset : offset + stored_bytes])
        except zstandard.ZstdError as error:
            raise ValueError("invalid zstd section") from error
        if len(decoded) != decoded_bytes or _crc32c(decoded) != checksum:
            raise ValueError("invalid section integrity")
        if kind == b"XTRA":
            _validate_extra(decoded, flags, (z, x, y), owners)
        sections.append((kind, (z, x, y), decoded))
    return sections


def parse_graph(
    payload: bytes,
) -> tuple[list[tuple[int, int, int]], list[tuple[int, ...]], list[tuple[int, int, int]]]:
    """Decode a GRPH payload into its nodes, edges, and geometry references."""
    node_count, edge_count, _adjacency_count, ref_count = struct.unpack_from("<4I", payload)
    nodes_offset, edges_offset, _csr_offset, _adjacency_offset, refs_offset = struct.unpack_from("<5I", payload, 16)
    node_ids_offset, edge_ids_offset = struct.unpack_from("<2I", payload, 36)
    node_ids = _identities(payload, node_ids_offset, node_count)
    edge_ids = _identities(payload, edge_ids_offset, edge_count)
    # Identity is rejoined with its record here so callers keep reading (id, ...) tuples: where
    # the bytes sit is the format's business, not theirs.
    nodes = [
        (node_ids[index], *struct.unpack_from("<II", payload, nodes_offset + index * 8)) for index in range(node_count)
    ]
    edges = [
        (edge_ids[index], *struct.unpack_from("<IIIIII", payload, edges_offset + index * 24))
        for index in range(edge_count)
    ]
    refs = [struct.unpack_from("<IIb", payload, refs_offset + index * 12) for index in range(ref_count)]
    return nodes, edges, refs


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
    first_vertices = itertools.accumulate((len(path) for _edge_index, path in paths), initial=0)
    path_headers = b"".join(
        struct.pack("<4I", edge_index, first_vertex, len(path), 0)
        for (edge_index, path), first_vertex in zip(paths, first_vertices)
    )
    vertices = b"".join(struct.pack("<HH", *point) for _edge_index, path in paths for point in path)
    header_size = 32
    paths_offset = header_size
    vertices_offset = paths_offset + len(path_headers)
    # SPEC 3.3: lines carry no mesh, so both mesh arrays are empty and start past the vertices.
    mesh_offset = vertices_offset + len(vertices)
    vertex_count = sum(len(path) for _edge_index, path in paths)
    header = struct.pack("<8I", len(paths), vertex_count, 0, 0, paths_offset, vertices_offset, mesh_offset, mesh_offset)
    return header + path_headers + vertices


def _node_order(edges: list[tuple[int, int, int]]) -> list[int]:
    """The node array, sorted by stable ID as SPEC 4 requires. Property rows follow it."""
    return sorted({node_id for _edge_id, source_id, target_id in edges for node_id in (source_id, target_id)})


def _graph_section(edges: list[tuple[int, int, int]], geometry_refs: list[list[tuple[int, int]]]) -> bytes:
    node_ids = _node_order(edges)
    node_indices = {node_id: index for index, node_id in enumerate(node_ids)}
    adjacency: list[list[tuple[int, int]]] = [[] for _node_id in node_ids]
    for edge_index, (_edge_id, source_id, target_id) in enumerate(edges):
        adjacency[node_indices[source_id]].append((edge_index, 1))
        adjacency[node_indices[target_id]].append((edge_index, -1))
    ref_starts = itertools.accumulate((len(refs) for refs in geometry_refs), initial=0)
    ref_count = sum(len(refs) for refs in geometry_refs)
    header_size = 44
    nodes_offset = header_size
    edges_offset = nodes_offset + len(node_ids) * 8
    csr_offset = edges_offset + len(edges) * 24
    adjacency_offset = csr_offset + (len(node_ids) + 1) * 8
    adjacency_count = sum(len(entries) for entries in adjacency)
    refs_offset = adjacency_offset + adjacency_count * 8
    node_id_block = _identity_block(node_ids)
    node_ids_offset = refs_offset + ref_count * 12
    edge_ids_offset = node_ids_offset + len(node_id_block)
    header = struct.pack(
        "<11I", len(node_ids), len(edges), adjacency_count, ref_count, nodes_offset, edges_offset, csr_offset, adjacency_offset, refs_offset, node_ids_offset, edge_ids_offset
    )
    nodes = b"".join(struct.pack("<II", index, 0) for index in range(len(node_ids)))
    edge_records = b"".join(
        struct.pack(
            "<IIIIII", node_indices[source_id], node_indices[target_id], edge_index, ref_start, len(refs), 0
        )
        for edge_index, ((_edge_id, source_id, target_id), ref_start, refs) in enumerate(zip(edges, ref_starts, geometry_refs))
    )
    offsets = [0]
    for entries in adjacency:
        offsets.append(offsets[-1] + len(entries))
    csr = struct.pack(f"<{len(offsets)}Q", *offsets)
    adjacency_records = b"".join(struct.pack("<Ib3x", edge_index, direction) for entries in adjacency for edge_index, direction in entries)
    ref_records = b"".join(
        struct.pack("<IIb3x", directory_index, path_index, 1) for refs in geometry_refs for directory_index, path_index in refs
    )
    # SPEC 4.1: identity is the only part of this section a compressor cannot help with, so it
    # is stored as gaps between sorted values rather than as eight bytes of hash each.
    return (
        header + nodes + edge_records + csr + adjacency_records + ref_records
        + node_id_block + _identity_block([edge_id for edge_id, *_rest in edges])
    )


def _identity_block(identities: list[int]) -> bytes:
    """Ascending ids as unsigned LEB128 gaps, the first counted from zero (SPEC 4.1)."""
    block = bytearray()
    previous = 0
    for identity in identities:
        gap = identity - previous
        while True:
            group, gap = gap & 0x7F, gap >> 7
            block.append(group | (0x80 if gap else 0))
            if not gap:
                break
        previous = identity
    return bytes(block)


def _identities(payload: bytes, offset: int, count: int) -> list[int]:
    identities = []
    value = 0
    at = offset
    for _index in range(count):
        gap, shift = 0, 0
        while True:
            if at >= len(payload):
                raise ValueError("identity block runs past the section")
            group = payload[at]
            at += 1
            gap |= (group & 0x7F) << shift
            shift += 7
            if not group & 0x80:
                break
        value += gap
        identities.append(value)
    return identities


def _property_section(node_rows: list[dict[str, Any]], edge_rows: list[dict[str, Any]]) -> bytes:
    """Typed nullable columns for both entity kinds, sharing one key dictionary (SPEC 5).

    A key gets a column only for the kind that uses it: an all-absent column would claim that
    edges have an elevation and merely never said which.
    """
    groups = [(1, node_rows), (2, edge_rows)]
    used = [(kind, rows, sorted({key for row in rows for key, value in row.items() if value is not None})) for kind, rows in groups]
    keys = sorted({key for _kind, _rows, group_keys in used for key in group_keys})
    key_ids = {key: index for index, key in enumerate(keys)}
    string_values = sorted({value for _kind, rows in groups for row in rows for value in row.values() if isinstance(value, str)})
    key_dictionary = _string_dictionary(keys)
    string_dictionary = _string_dictionary(string_values)
    enum_dictionary = struct.pack("<I", 0)
    header_size = 40
    key_offset = header_size
    string_offset = key_offset + len(key_dictionary)
    enum_offset = string_offset + len(string_dictionary)
    columns_offset = enum_offset + len(enum_dictionary)
    values_offset = columns_offset + sum(len(group_keys) for _kind, _rows, group_keys in used) * 20
    columns: list[bytes] = []
    bodies: list[bytes] = []
    for kind, rows, group_keys in used:
        bitmap_bytes = (len(rows) + 7) // 8
        for key in group_keys:
            present = [index for index, row in enumerate(rows) if row.get(key) is not None]
            values = [rows[index][key] for index in present]
            value_type = _column_type(key, values)
            presence_offset = values_offset + sum(len(body) for body in bodies)
            columns.append(
                struct.pack(
                    "<IBBHIII", key_ids[key], kind, value_type, 1, len(rows), presence_offset, presence_offset + bitmap_bytes
                )
            )
            bodies.append(_packed_bits(present, len(rows)) + _column_values(value_type, values, string_values))
    node_columns = len(used[0][2])
    header = struct.pack(
        "<10I", len(keys), len(string_values), 0, node_columns, len(used[1][2]), key_offset, string_offset, enum_offset, columns_offset, columns_offset + node_columns * 20
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
        string_ids = {value: index for index, value in enumerate(string_values)}
        return b"".join(struct.pack("<I", string_ids[value]) for value in values)
    if value_type == 2:
        return b"".join(struct.pack("<q", value) for value in values)
    return b"".join(struct.pack("<d", value) for value in values)


_ENTITY_KINDS = {"node": 1, "edge": 2}


def _state_channel_section(channels: list[dict[str, Any]]) -> bytes:
    """Encode STCH per SPEC section 6. The file declares what solvers may write, never samples."""
    strings: list[str] = []

    def string_id(value: str) -> int:
        if value not in strings:
            strings.append(value)
        return strings.index(value)

    records = []
    for index, channel in enumerate(channels):
        name = str(channel["name"])
        entity_kind = _ENTITY_KINDS.get(str(channel.get("entity_kind", "edge")))
        if entity_kind is None:
            raise ValueError(f"channel {name!r} must apply to a node or an edge")
        minimum, maximum = channel.get("min"), channel.get("max")
        if (minimum is None) != (maximum is None):
            raise ValueError(f"channel {name!r} declares half a range")
        if minimum is not None and maximum is not None and float(minimum) > float(maximum):
            raise ValueError(f"channel {name!r} declares an inverted range")
        flags = (1 if minimum is not None else 0) | (2 if channel.get("interpolate", True) else 0)
        records.append(
            struct.pack(
                "<HBBIIffI",
                index + 1,  # SPEC 6: ids are unique and non-zero
                entity_kind,
                1,
                string_id(name),
                string_id(str(channel.get("unit", "1"))),
                0.0 if minimum is None else float(minimum),
                0.0 if maximum is None else float(maximum),
                flags,
            )
        )

    header_size = 12
    string_table = _string_dictionary(strings)
    return struct.pack("<3I", len(records), header_size, header_size + len(string_table)) + string_table + b"".join(records)


def _extra_sections(extras: list[Extra]) -> list[bytes]:
    """Encode SPEC 7 records, sorted so the same inputs produce the same file."""
    ordered = sorted(extras, key=lambda extra: (extra.owner, extra.media_type))
    for previous, current in itertools.pairwise(ordered):
        if (previous.owner, previous.media_type) == (current.owner, current.media_type):
            raise ValueError(f"two XTRA records share an owner and media type: {current.owner!r}, {current.media_type!r}")
    payloads = []
    for extra in ordered:
        if not _OWNER.fullmatch(extra.owner):
            raise ValueError(f"XTRA owner must be a solver id of lowercase letters, digits and '-', got {extra.owner!r}")
        if not extra.media_type:
            raise ValueError(f"XTRA record owned by {extra.owner!r} declares no media type")
        owner, media_type = extra.owner.encode(), extra.media_type.encode()
        owner_offset = _EXTRA_HEADER.size
        media_offset = owner_offset + len(owner)
        payload_offset = media_offset + len(media_type)
        header = _EXTRA_HEADER.pack(
            owner_offset, len(owner), media_offset, len(media_type), payload_offset, len(extra.payload), 0
        )
        payloads.append(header + owner + media_type + extra.payload)
    return payloads


def _validate_extra(payload: bytes, flags: int, key: tuple[int, int, int], seen: set[tuple[str, str]]) -> None:
    """Check what a reader relies on but cannot recover if a writer got it wrong (SPEC 7)."""
    if flags & 1:
        raise ValueError("an XTRA record must be optional, so an older reader can skip it")
    if key != (0, 0, 0):
        raise ValueError("an XTRA record is not tile-scoped, so its tile key must be zero")
    if len(payload) < _EXTRA_HEADER.size:
        raise ValueError("XTRA record is shorter than its header")
    owner_offset, owner_bytes, media_offset, media_bytes, body_offset, body_bytes, extra_flags = _EXTRA_HEADER.unpack_from(payload)
    spans = ((owner_offset, owner_bytes), (media_offset, media_bytes), (body_offset, body_bytes))
    if extra_flags or any(offset < _EXTRA_HEADER.size or offset + length > len(payload) for offset, length in spans):
        raise ValueError("invalid XTRA record header")
    identity = (payload[owner_offset : owner_offset + owner_bytes].decode(), payload[media_offset : media_offset + media_bytes].decode())
    if identity in seen:
        raise ValueError(f"two XTRA records share an owner and media type: {identity[0]!r}, {identity[1]!r}")
    seen.add(identity)


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


def _sources(source: Path) -> list[Path]:
    if not source.is_dir():
        return [source]
    paths = [source / name for name in ("edges.geojson", "nodes.geojson")]
    present = [path for path in paths if path.exists()]
    if not present:
        raise ValueError(f"{source} holds no edges.geojson or nodes.geojson")
    return present


def _declared(feature: dict[str, Any], derived_from: str) -> int:
    """The feature's declared identity, or one derived from its source name. `0` is a valid ID."""
    declared = _declared_id(feature)
    return _stable_id(derived_from) if declared is None else declared


def _declared_id(feature: dict[str, Any]) -> int | None:
    """Reads `_trama_id`, rejecting a malformed one rather than silently deriving a replacement."""
    declared = (feature.get("properties") or {}).get(_ID_KEY)
    if declared is None:
        return None
    try:
        value = int(declared)
    except (TypeError, ValueError) as error:
        raise ValueError(f"{_ID_KEY} must be a decimal integer, got {declared!r}") from error
    if not 0 <= value < 1 << 64:
        raise ValueError(f"{_ID_KEY} must fit in u64, got {declared!r}")
    return value


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
        max(0, min(_EXTENT, round((point[0] - min_x) / width * _EXTENT))),
        max(0, min(_EXTENT, round((max_y - point[1]) / width * _EXTENT))),
    )


def _node_cell(point: tuple[float, float]) -> tuple[int, int]:
    """The identity cell of a projected point: SPEC 4.2, the geometry grid made global.

    Quantization samples both tile edges, so `_EXTENT` in one tile addresses the position `0`
    addresses in the next, and a node on a tile boundary lands in one cell either way.
    """
    z, x, y = _tile_key(*point)
    quantized_x, quantized_y = _quantize(point, z, x, y)
    return x * _EXTENT + quantized_x, y * _EXTENT + quantized_y


def _crc32c_table() -> list[int]:
    table = []
    for byte in range(256):
        crc = byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
        table.append(crc)
    return table


_CRC_TABLE = _crc32c_table()


def _crc32c(data: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in data:
        crc = (crc >> 8) ^ _CRC_TABLE[(crc ^ byte) & 0xFF]
    return (~crc) & 0xFFFFFFFF
