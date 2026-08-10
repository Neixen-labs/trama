# SPDX-License-Identifier: LicenseRef-BSL-1.1
from pathlib import Path

from typer.testing import CliRunner

from trama_engine.cli import app


def test_compile_command_creates_output(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text(
        '{"type":"FeatureCollection","features":[{"type":"Feature","geometry":{"type":"LineString","coordinates":[[-3.704,40.416],[-3.703,40.417]]},"properties":{}}]}'
    )
    output = tmp_path / "network.trama"

    result = CliRunner().invoke(app, ["compile", str(source), str(output)])

    assert result.exit_code == 0, result.output
    assert output.exists()


def test_compile_command_reports_rejected_input(tmp_path: Path) -> None:
    source = tmp_path / "network.geojson"
    source.write_text('{"type":"FeatureCollection","features":[]}')

    result = CliRunner().invoke(app, ["compile", str(source), str(tmp_path / "network.trama")])

    assert result.exit_code == 1
    assert "no LineString features" in result.output
