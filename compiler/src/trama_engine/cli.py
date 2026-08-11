# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Command-line interface for the TRAMA compiler."""

import json
from pathlib import Path
from typing import Annotated

import typer

from trama_engine import importers
from trama_engine.compiler import compile_features, compile_geojson, validate_container
from trama_engine.exporter import export_geojson

app = typer.Typer(no_args_is_help=True)


@app.callback()
def main() -> None:
    """Compile source network data into TRAMA files."""


@app.command()
def compile(
    source: Path,
    destination: Path,
    channels: Annotated[Path | None, typer.Option(help="JSON list of state channels to declare.")] = None,
    option: Annotated[list[str] | None, typer.Option("--option", "-o", help="key=value passed to the importer.")] = None,
) -> None:
    """Compile GeoJSON, or any format an installed importer claims, into a `.trama` file."""
    try:
        declared = json.loads(channels.read_text()) if channels is not None else None
        options = importers.parse_options(option or [])
        if source.is_dir() or source.suffix.lower() in importers.NATIVE_SUFFIXES:
            compile_geojson(source, destination, declared)
        else:
            imported = _import(source, options)
            compile_features(imported.features, destination, declared, imported.extras)
    except (OSError, TypeError, ValueError, KeyError) as error:
        typer.echo(str(error), err=True)
        raise typer.Exit(1) from error


def _import(source: Path, options: dict[str, str]) -> importers.Import:
    importer = importers.find(source)
    if importer is None:
        raise ValueError(f"no installed importer claims {source.suffix!r}; install the package that reads it")
    return importer.load(source, options)


@app.command()
def validate(source: Path) -> None:
    """Validate a `.trama` container."""
    try:
        validate_container(source)
    except (OSError, ValueError) as error:
        typer.echo(str(error), err=True)
        raise typer.Exit(1) from error


@app.command()
def export(source: Path, destination: Path, to: str = typer.Option("geojson", help="Export format.")) -> None:
    """Export a `.trama` file into a directory of GeoJSON FeatureCollections."""
    if to != "geojson":
        typer.echo(f"unsupported export format {to!r}", err=True)
        raise typer.Exit(1)
    try:
        export_geojson(source, destination)
    except (OSError, ValueError) as error:
        typer.echo(str(error), err=True)
        raise typer.Exit(1) from error
