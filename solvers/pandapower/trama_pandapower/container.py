# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""A TRAMA reader written from ``docs/SPEC.md``, with nothing borrowed from ``trama-format``.

This module exists twice over. It is what the solver needs to read a network, and it is the
evidence for the claim the format is built on: that a second implementer, in another language,
can decode a file from the specification alone. Every offset below was read off the document
rather than off the Rust, which is why writing it found three things the document had not said.

It reads ``GRPH``, ``PROP``, ``STCH`` and ``XTRA``. ``GEOM`` is render geometry and a solver has
no use for it, so it is left in the file — which is the point of the section directory.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from typing import Any

import zstandard

MAGIC = b"TRAMA\0\0\0"
HEADER_BYTES = 64
DIRECTORY_RECORD_BYTES = 64
#: This reader implements v0. SPEC 2.1 has it reject anything demanding more.
SUPPORTED_VERSION = (0, 9, 9)


class MalformedContainer(Exception):
    """The bytes are not a TRAMA file this reader can decode."""


def _crc32c_table() -> list[int]:
    """Castagnoli, the polynomial SPEC 2.2 names. ``zlib.crc32`` is the other CRC-32."""
    table = []
    for byte in range(256):
        crc = byte
        for _ in range(8):
            crc = (crc >> 1) ^ (0x82F63B78 if crc & 1 else 0)
        table.append(crc)
    return table


_CRC32C = _crc32c_table()


def crc32c(payload: bytes) -> int:
    crc = 0xFFFFFFFF
    for byte in payload:
        crc = (crc >> 8) ^ _CRC32C[(crc ^ byte) & 0xFF]
    return crc ^ 0xFFFFFFFF


@dataclass(frozen=True)
class Section:
    kind: bytes
    key: tuple[int, int, int]
    offset: int
    stored_bytes: int
    uncompressed_bytes: int
    crc32c: int


@dataclass(frozen=True)
class Extra:
    """SPEC 7. A reader that does not know the owner ignores the record; it never rejects it."""

    owner: str
    media_type: str
    payload: bytes


@dataclass(frozen=True)
class Channel:
    channel_id: int
    entity_kind: int
    name: str
    unit: str
    declared_min: float
    declared_max: float
    range_present: bool


@dataclass(frozen=True)
class Graph:
    node_ids: list[int]
    edge_ids: list[int]
    #: Per edge, the indices of its endpoints in the node arrays.
    edge_endpoints: list[tuple[int, int]]
    node_property_rows: list[int]
    edge_property_rows: list[int]


class Container:
    """One ``.trama`` file, decoded on demand."""

    def __init__(self, data: bytes) -> None:
        self._data = data
        if data[:8] != MAGIC:
            raise MalformedContainer("not a TRAMA file: the magic is wrong")
        minimum = struct.unpack_from("<3H", data, 0x0E)
        if minimum > SUPPORTED_VERSION:
            raise MalformedContainer(f"the file demands a reader of version {minimum}")
        header_bytes, directory_offset, section_count = struct.unpack_from("<IQI", data, 0x14)
        if header_bytes != HEADER_BYTES:
            raise MalformedContainer(f"v0 headers are 64 bytes, not {header_bytes}")
        file_bytes = struct.unpack_from("<Q", data, 0x28)[0]
        if file_bytes != len(data):
            raise MalformedContainer(f"the file declares {file_bytes} bytes and is {len(data)}")
        self.sections = [self._directory_record(directory_offset + index * DIRECTORY_RECORD_BYTES) for index in range(section_count)]

    def _directory_record(self, at: int) -> Section:
        kind = self._data[at : at + 4]
        key = struct.unpack_from("<3I", self._data, at + 0x08)
        offset, stored, uncompressed = struct.unpack_from("<3Q", self._data, at + 0x14)
        checksum, codec = struct.unpack_from("<IH", self._data, at + 0x2C)
        if codec != 1:
            raise MalformedContainer(f"v0 permits zstd only, not codec {codec}")
        return Section(kind, key, offset, stored, uncompressed, checksum)

    def payload(self, section: Section) -> bytes:
        """One section, decompressed and checked. SPEC 2.2 requires both checks of a reader."""
        frame = self._data[section.offset : section.offset + section.stored_bytes]
        decoded = zstandard.ZstdDecompressor().decompress(frame)
        if len(decoded) != section.uncompressed_bytes:
            raise MalformedContainer(f"{section.kind!r} decodes to {len(decoded)} bytes, not {section.uncompressed_bytes}")
        if crc32c(decoded) != section.crc32c:
            raise MalformedContainer(f"{section.kind!r} fails its CRC-32C")
        return decoded

    def _only(self, kind: bytes) -> bytes:
        found = [section for section in self.sections if section.kind == kind]
        if len(found) != 1:
            raise MalformedContainer(f"a file has exactly one {kind!r} record, not {len(found)}")
        return self.payload(found[0])

    def graph(self) -> Graph:
        return _read_graph(self._only(b"GRPH"))

    def channels(self) -> list[Channel]:
        return _read_channels(self._only(b"STCH"))

    def properties(self) -> tuple[dict[str, list[Any]], dict[str, list[Any]]]:
        """Node and edge property columns by key, each a list with ``None`` where absent."""
        return _read_properties(self._only(b"PROP"))

    def extras(self) -> list[Extra]:
        return [_read_extra(self.payload(section)) for section in self.sections if section.kind == b"XTRA"]


def _varints(payload: bytes, at: int, count: int) -> tuple[list[int], int]:
    """SPEC 4.1: the first value, then the gap to each predecessor, as unsigned LEB128."""
    values: list[int] = []
    running = 0
    for index in range(count):
        shift = 0
        value = 0
        while True:
            if at >= len(payload):
                raise MalformedContainer("an identity block runs past its section")
            byte = payload[at]
            at += 1
            value |= (byte & 0x7F) << shift
            if not byte & 0x80:
                break
            shift += 7
        if index > 0 and value == 0:
            raise MalformedContainer("identities ascend, so a gap of zero is malformed")
        running = value if index == 0 else running + value
        values.append(running)
    return values, at


def _read_graph(payload: bytes) -> Graph:
    (
        node_count,
        edge_count,
        _adjacency_count,
        _geometry_ref_count,
        nodes_offset,
        edges_offset,
        _csr_offsets_offset,
        _adjacency_offset,
        _geometry_refs_offset,
        node_ids_offset,
        edge_ids_offset,
    ) = struct.unpack_from("<11I", payload, 0)

    node_rows = [struct.unpack_from("<I", payload, nodes_offset + index * 8)[0] for index in range(node_count)]
    edges = [struct.unpack_from("<6I", payload, edges_offset + index * 24) for index in range(edge_count)]
    node_ids, _ = _varints(payload, node_ids_offset, node_count)
    edge_ids, _ = _varints(payload, edge_ids_offset, edge_count)
    return Graph(
        node_ids=node_ids,
        edge_ids=edge_ids,
        edge_endpoints=[(edge[0], edge[1]) for edge in edges],
        node_property_rows=node_rows,
        edge_property_rows=[edge[2] for edge in edges],
    )


def _read_strings(payload: bytes, at: int) -> list[str]:
    """SPEC 5: ``u32 count``, then each string as a ``u32`` byte length and its UTF-8 bytes."""
    count = struct.unpack_from("<I", payload, at)[0]
    at += 4
    values = []
    for _ in range(count):
        length = struct.unpack_from("<I", payload, at)[0]
        at += 4
        values.append(payload[at : at + length].decode("utf-8"))
        at += length
    return values


def _bit(bitmap: bytes, index: int) -> bool:
    """SPEC 5: entity ``i`` is bit ``i mod 8`` of byte ``i div 8``, least significant first."""
    return bool(bitmap[index // 8] & (1 << (index % 8)))


def _read_properties(payload: bytes) -> tuple[dict[str, list[Any]], dict[str, list[Any]]]:
    (
        _key_count,
        _string_count,
        _enum_count,
        node_column_count,
        edge_column_count,
        key_dictionary_offset,
        string_dictionary_offset,
        enum_dictionary_offset,
        node_columns_offset,
        _edge_columns_offset,
    ) = struct.unpack_from("<10I", payload, 0)
    keys = _read_strings(payload, key_dictionary_offset)
    strings = _read_strings(payload, string_dictionary_offset)
    enums = _read_strings(payload, enum_dictionary_offset)

    by_kind: tuple[dict[str, list[Any]], dict[str, list[Any]]] = ({}, {})
    for index in range(node_column_count + edge_column_count):
        at = node_columns_offset + index * 20
        key_id, entity_kind, value_type, _flags, entity_count, presence_offset, values_offset = struct.unpack_from(
            "<IBBHIII", payload, at
        )
        bitmap = payload[presence_offset : presence_offset + (entity_count + 7) // 8]
        present = [row for row in range(entity_count) if _bit(bitmap, row)]
        column: list[Any] = [None] * entity_count
        for slot, row in enumerate(present):
            column[row] = _value(payload, values_offset, slot, value_type, strings, enums)
        by_kind[0 if entity_kind == 1 else 1][keys[key_id]] = column
    return by_kind


def _value(payload: bytes, values_offset: int, slot: int, value_type: int, strings: list[str], enums: list[str]) -> Any:
    if value_type == 1:
        return struct.unpack_from("<d", payload, values_offset + slot * 8)[0]
    if value_type == 2:
        return struct.unpack_from("<q", payload, values_offset + slot * 8)[0]
    if value_type == 3:
        return strings[struct.unpack_from("<I", payload, values_offset + slot * 4)[0]]
    if value_type == 4:
        # A bitmap over the present values, not over every entity.
        return _bit(payload[values_offset:], slot)
    if value_type == 5:
        return enums[struct.unpack_from("<I", payload, values_offset + slot * 4)[0]]
    raise MalformedContainer(f"unknown property value type {value_type}")


def _read_channels(payload: bytes) -> list[Channel]:
    channel_count, strings_offset, channels_offset = struct.unpack_from("<3I", payload, 0)
    strings = _read_strings(payload, strings_offset)
    channels = []
    for index in range(channel_count):
        at = channels_offset + index * 24
        channel_id, entity_kind, value_type, name_id, unit_id, low, high, flags = struct.unpack_from(
            "<HBBIIffI", payload, at
        )
        if value_type != 1:
            raise MalformedContainer("v0 state channels are scalar f32")
        channels.append(
            Channel(channel_id, entity_kind, strings[name_id], strings[unit_id], low, high, bool(flags & 1))
        )
    return channels


def _read_extra(payload: bytes) -> Extra:
    owner_offset, owner_bytes, media_offset, media_bytes, payload_offset, payload_bytes, _flags = struct.unpack_from(
        "<7I", payload, 0
    )
    return Extra(
        owner=payload[owner_offset : owner_offset + owner_bytes].decode("utf-8"),
        media_type=payload[media_offset : media_offset + media_bytes].decode("utf-8"),
        payload=payload[payload_offset : payload_offset + payload_bytes],
    )
