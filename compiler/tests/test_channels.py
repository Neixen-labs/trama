# SPDX-License-Identifier: LicenseRef-BSL-1.1
import json
import struct
from pathlib import Path

import pytest
from typer.testing import CliRunner

from trama_engine.cli import app
from trama_engine.compiler import compile_geojson, read_sections

NETWORK = (
    '{"type":"FeatureCollection","features":[{"type":"Feature","id":"a","properties":{},'
    '"geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]}}]}'
)


def _state_section(container: Path) -> bytes:
    return next(payload for kind, _key, payload in read_sections(container.read_bytes()) if kind == b"STCH")


def _strings(payload: bytes, offset: int) -> list[str]:
    count = struct.unpack_from("<I", payload, offset)[0]
    values, at = [], offset + 4
    for _index in range(count):
        length = struct.unpack_from("<I", payload, at)[0]
        values.append(payload[at + 4 : at + 4 + length].decode())
        at += 4 + length
    return values


def _compile(tmp_path: Path, channels: list[dict] | None) -> Path:
    source = tmp_path / "network.geojson"
    source.write_text(NETWORK)
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination, channels)
    return destination


def test_declared_channels_match_the_spec_layout(tmp_path: Path) -> None:
    destination = _compile(
        tmp_path,
        [
            {"name": "pressure", "entity_kind": "node", "unit": "m", "min": 0, "max": 80},
            {"name": "flow", "entity_kind": "edge", "unit": "l/s", "interpolate": False},
        ],
    )

    payload = _state_section(destination)
    channel_count, strings_offset, channels_offset = struct.unpack_from("<3I", payload)
    strings = _strings(payload, strings_offset)
    assert channel_count == 2
    first = struct.unpack_from("<HBBIIffI", payload, channels_offset)
    second = struct.unpack_from("<HBBIIffI", payload, channels_offset + 24)
    # id, entity_kind, value_type, name, unit, min, max, flags
    assert first == (1, 1, 1, strings.index("pressure"), strings.index("m"), 0.0, 80.0, 0b11)
    assert second == (2, 2, 1, strings.index("flow"), strings.index("l/s"), 0.0, 0.0, 0b00)


def test_channel_ids_are_unique_and_non_zero(tmp_path: Path) -> None:
    destination = _compile(tmp_path, [{"name": f"channel-{index}"} for index in range(4)])

    payload = _state_section(destination)
    _count, _strings_offset, channels_offset = struct.unpack_from("<3I", payload)
    ids = [struct.unpack_from("<H", payload, channels_offset + index * 24)[0] for index in range(4)]
    assert ids == [1, 2, 3, 4]


def test_a_container_without_declarations_still_writes_a_legal_section(tmp_path: Path) -> None:
    payload = _state_section(_compile(tmp_path, None))

    channel_count, strings_offset, _channels_offset = struct.unpack_from("<3I", payload)
    assert channel_count == 0
    # SPEC 6: strings_offset must address a u32 count even when the table is empty.
    assert _strings(payload, strings_offset) == []


@pytest.mark.parametrize(
    ("channel", "message"),
    [
        ({"name": "x", "min": 10, "max": 1}, "inverted range"),
        ({"name": "x", "min": 10}, "half a range"),
        ({"name": "x", "entity_kind": "pipe"}, "node or an edge"),
    ],
)
def test_invalid_declarations_are_rejected(tmp_path: Path, channel: dict, message: str) -> None:
    with pytest.raises(ValueError, match=message):
        _compile(tmp_path, [channel])


def test_compile_command_reads_channels_from_a_file(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(NETWORK)
    channels = tmp_path / "channels.json"
    channels.write_text(json.dumps([{"name": "pressure", "unit": "m", "min": 0, "max": 80}]))
    destination = tmp_path / "network.trama"

    result = CliRunner().invoke(app, ["compile", str(source), str(destination), "--channels", str(channels)])

    assert result.exit_code == 0, result.output
    assert struct.unpack_from("<I", _state_section(destination))[0] == 1
