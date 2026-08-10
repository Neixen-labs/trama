# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Command-line interface for the TRAMA compiler."""

from pathlib import Path

import typer

from trama_engine.compiler import compile_geojson

app = typer.Typer(no_args_is_help=True)


@app.callback()
def main() -> None:
    """Compile source network data into TRAMA files."""


@app.command()
def compile(source: Path, destination: Path) -> None:
    """Compile one GeoJSON LineString feature to a `.trama` file."""
    try:
        compile_geojson(source, destination)
    except (OSError, ValueError) as error:
        raise typer.Exit(str(error)) from error
