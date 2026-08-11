# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Opaque owner-scoped records, per SPEC 7."""

import struct
from pathlib import Path

import pytest

from trama_engine.compiler import (
    Extra,
    compile_geojson,
    read_sections,
    validate_container,
)

SOURCE = (
    '{"type":"FeatureCollection","features":[{"type":"Feature","id":"a","properties":{"loss":1.5},'
    '"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
)


def _compile(tmp_path: Path, extras: list[Extra] | None = None) -> Path:
    tmp_path.mkdir(parents=True, exist_ok=True)
    source = tmp_path / "network.geojson"
    source.write_text(SOURCE)
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination, extras=extras)
    return destination


def _unpack(payload: bytes) -> tuple[str, str, bytes]:
    owner_offset, owner_bytes, media_offset, media_bytes, body_offset, body_bytes, flags = struct.unpack_from(
        "<7I", payload
    )
    assert flags == 0
    return (
        payload[owner_offset : owner_offset + owner_bytes].decode(),
        payload[media_offset : media_offset + media_bytes].decode(),
        payload[body_offset : body_offset + body_bytes],
    )


def test_an_extra_survives_the_round_trip_unread(tmp_path: Path) -> None:
    container = _compile(tmp_path, [Extra("epanet", "application/octet-stream", b"\x00 not utf-8 \xff")])

    sections = read_sections(container.read_bytes())
    extras = [(key, payload) for kind, key, payload in sections if kind == b"XTRA"]
    assert len(extras) == 1
    key, payload = extras[0]
    assert key == (0, 0, 0)  # SPEC 7: an XTRA record is not tile-scoped
    assert _unpack(payload) == ("epanet", "application/octet-stream", b"\x00 not utf-8 \xff")


def test_an_extra_is_additive_and_nothing_else_moves(tmp_path: Path) -> None:
    # SPEC 7.1: dropping every XTRA record must leave the same file. The strongest check the
    # compiler can make of that is that adding one changed no other payload.
    plain = read_sections(_compile(tmp_path / "plain").read_bytes())
    carrying = read_sections(_compile(tmp_path / "with", [Extra("epanet", "text/plain", b"[PATTERNS]")]).read_bytes())

    assert [record for record in carrying if record[0] != b"XTRA"] == plain


def test_an_extra_is_written_optional_so_an_older_reader_skips_it(tmp_path: Path) -> None:
    data = _compile(tmp_path, [Extra("epanet", "text/plain", b"x")]).read_bytes()

    flags = {
        struct.unpack_from("<4s", data, 64 + index * 64)[0]: struct.unpack_from("<I", data, 64 + index * 64 + 4)[0]
        for index in range(struct.unpack_from("<I", data, 0x20)[0])
    }
    assert flags[b"XTRA"] & 1 == 0
    assert all(value & 1 for kind, value in flags.items() if kind != b"XTRA")


def test_two_records_with_the_same_owner_and_media_type_are_rejected(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="owner and media type"):
        _compile(tmp_path, [Extra("epanet", "text/plain", b"one"), Extra("epanet", "text/plain", b"two")])


def test_the_same_owner_may_carry_two_media_types(tmp_path: Path) -> None:
    container = _compile(tmp_path, [Extra("epanet", "text/plain", b"one"), Extra("epanet", "text/csv", b"two")])

    validate_container(container)
    assert sum(1 for kind, _key, _payload in read_sections(container.read_bytes()) if kind == b"XTRA") == 2


def test_extras_are_ordered_so_the_file_stays_reproducible(tmp_path: Path) -> None:
    first = _compile(tmp_path, [Extra("b-owner", "text/plain", b"two"), Extra("a-owner", "text/plain", b"one")])
    second = _compile(tmp_path / "b", [Extra("a-owner", "text/plain", b"one"), Extra("b-owner", "text/plain", b"two")])

    assert first.read_bytes() == second.read_bytes()


def test_an_owner_that_is_not_a_solver_id_is_rejected(tmp_path: Path) -> None:
    # SOLVER_CONTRACT 2: lowercase ASCII, digits and `-`. An owner nothing can claim is a
    # payload nothing will ever read back.
    with pytest.raises(ValueError, match="owner"):
        _compile(tmp_path, [Extra("EPANET 2.2", "text/plain", b"x")])


def test_a_required_extra_is_rejected_on_validation(tmp_path: Path) -> None:
    container = _compile(tmp_path, [Extra("epanet", "text/plain", b"x")])
    data = bytearray(container.read_bytes())
    for index in range(struct.unpack_from("<I", data, 0x20)[0]):
        record = 64 + index * 64
        if data[record : record + 4] == b"XTRA":
            struct.pack_into("<I", data, record + 4, 1)
    container.write_bytes(data)

    with pytest.raises(ValueError, match="optional"):
        validate_container(container)
