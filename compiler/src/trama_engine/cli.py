# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Command-line interface for the TRAMA compiler."""

from enum import Enum
from pathlib import Path

import typer

from trama_engine.compiler import compile_file, export_file

app = typer.Typer(no_args_is_help=True)


class Target(str, Enum):
    """Export targets. `SPEC.md` section 8 also defines gpkg and mvt; neither exists yet."""

    geojson = "geojson"
    epanet = "epanet"


@app.callback()
def main() -> None:
    """Compile source network data into TRAMA files."""


@app.command()
def compile(source: Path, destination: Path) -> None:
    """Compile a `.geojson` or EPANET `.inp` file to a `.trama` file."""
    _run(lambda: compile_file(source, destination))


@app.command()
def export(source: Path, destination: Path, to: Target = Target.geojson) -> None:
    """Export a `.trama` file back to GeoJSON or EPANET."""
    for path in _run(lambda: export_file(source, destination, to.value)) or ():
        typer.echo(path)


def _run(action):
    try:
        return action()
    except (OSError, ValueError) as error:
        typer.echo(f"error: {error}", err=True)
        raise typer.Exit(1) from error
