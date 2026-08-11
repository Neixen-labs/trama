# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The compiler routes unknown suffixes to an installed importer and knows no format itself."""

import json
import struct
from collections.abc import Mapping
from pathlib import Path

import pytest
from typer.testing import CliRunner

from trama_engine import importers
from trama_engine.cli import app
from trama_engine.compiler import Extra, read_sections
from trama_engine.importers import Import

runner = CliRunner()

LINE = {
    "type": "Feature",
    "id": "a",
    "properties": {"loss": 1.5},
    "geometry": {"type": "LineString", "coordinates": [[-3.704, 40.416], [-3.703, 40.417]]},
}


class FakeImporter:
    """Stands in for solvers/epanet: a format the core knows nothing about."""

    suffixes = (".fake",)

    def load(self, source: Path, options: Mapping[str, str]) -> Import:
        if "source-crs" not in options:
            raise ValueError("a .fake file declares no coordinate reference system; pass -o source-crs=...")
        return Import(
            features=[LINE],
            extras=[Extra("fake-solver", "text/plain", source.read_bytes())],
            channels=[{"name": "pressure", "entity_kind": "node", "unit": "m"}],
        )


@pytest.fixture
def installed(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(importers, "_installed", lambda: [FakeImporter()])


def test_an_importer_supplies_both_features_and_opaque_records(tmp_path: Path, installed: None) -> None:
    source = tmp_path / "network.fake"
    source.write_text("whatever this format is")
    destination = tmp_path / "network.trama"

    result = runner.invoke(app, ["compile", str(source), str(destination), "-o", "source-crs=EPSG:25830"])

    assert result.exit_code == 0, result.output
    sections = read_sections(destination.read_bytes())
    assert [kind for kind, _key, _payload in sections].count(b"XTRA") == 1
    assert b"whatever this format is" in next(payload for kind, _key, payload in sections if kind == b"XTRA")


def test_options_reach_the_importer_and_its_refusal_reaches_the_user(tmp_path: Path, installed: None) -> None:
    source = tmp_path / "network.fake"
    source.write_text("whatever this format is")

    result = runner.invoke(app, ["compile", str(source), str(tmp_path / "out.trama")])

    assert result.exit_code == 1
    assert "coordinate reference system" in result.output


def test_an_unclaimed_suffix_says_what_is_missing(tmp_path: Path) -> None:
    source = tmp_path / "network.inp"
    source.write_text("[JUNCTIONS]")

    result = runner.invoke(app, ["compile", str(source), str(tmp_path / "out.trama")])

    assert result.exit_code == 1
    assert ".inp" in result.output and "importer" in result.output


def test_geojson_still_compiles_with_no_importer_installed(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(json.dumps({"type": "FeatureCollection", "features": [LINE]}))
    destination = tmp_path / "network.trama"

    result = runner.invoke(app, ["compile", str(source), str(destination)])

    assert result.exit_code == 0, result.output
    assert [kind for kind, _key, _payload in read_sections(destination.read_bytes())].count(b"XTRA") == 0


def test_a_malformed_option_is_rejected_before_anything_is_written(tmp_path: Path, installed: None) -> None:
    source = tmp_path / "network.fake"
    source.write_text("whatever")
    destination = tmp_path / "network.trama"

    result = runner.invoke(app, ["compile", str(source), str(destination), "-o", "source-crs"])

    assert result.exit_code == 1
    assert "key=value" in result.output
    assert not destination.exists()


def _channel_names(container: Path) -> list[str]:
    payload = next(p for kind, _key, p in read_sections(container.read_bytes()) if kind == b"STCH")
    count, strings_offset, _records_offset = struct.unpack_from("<3I", payload)
    names = []
    at = strings_offset + 4
    for _index in range(struct.unpack_from("<I", payload, strings_offset)[0]):
        length = struct.unpack_from("<I", payload, at)[0]
        names.append(payload[at + 4 : at + 4 + length].decode())
        at += 4 + length
    return names[: count * 2 : 2]


def test_an_importer_declares_what_its_format_can_be_solved_for(tmp_path: Path, installed: None) -> None:
    source = tmp_path / "network.fake"
    source.write_text("whatever")
    destination = tmp_path / "network.trama"

    result = runner.invoke(app, ["compile", str(source), str(destination), "-o", "source-crs=EPSG:25830"])

    assert result.exit_code == 0, result.output
    assert _channel_names(destination) == ["pressure"]


def test_an_explicit_channels_file_wins_over_the_importer(tmp_path: Path, installed: None) -> None:
    source = tmp_path / "network.fake"
    source.write_text("whatever")
    channels = tmp_path / "channels.json"
    channels.write_text(json.dumps([{"name": "head", "entity_kind": "node", "unit": "m"}]))
    destination = tmp_path / "network.trama"

    result = runner.invoke(
        app,
        ["compile", str(source), str(destination), "-o", "source-crs=EPSG:25830", "--channels", str(channels)],
    )

    assert result.exit_code == 0, result.output
    assert _channel_names(destination) == ["head"]
