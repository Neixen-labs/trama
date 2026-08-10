# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Command-line interface for the TRAMA compiler."""

from enum import Enum
from pathlib import Path

import typer

from trama_engine.compiler import compile_geojson
from trama_engine.export import export_geojson

app = typer.Typer(no_args_is_help=True)


class Target(str, Enum):
    """Export targets. `SPEC.md` section 8 also defines gpkg and mvt; neither exists yet."""

    geojson = "geojson"


@app.callback()
def main() -> None:
    """Compile source network data into TRAMA files."""


@app.command()
def compile(source: Path, destination: Path) -> None:
    """Compile GeoJSON LineString features to a `.trama` file."""
    _run(compile_geojson, source, destination)


@app.command()
def export(source: Path, destination: Path, to: Target = Target.geojson) -> None:
    """Export a `.trama` file to `<destination>.nodes.geojson` and `<destination>.edges.geojson`."""
    for path in _run(export_geojson, source, destination):
        typer.echo(path)


def _run(action, source: Path, destination: Path):
    try:
        return action(source, destination)
    except (OSError, ValueError) as error:
        typer.echo(f"error: {error}", err=True)
        raise typer.Exit(1) from error
