# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Pick an input or output adapter and run it."""

from __future__ import annotations

from pathlib import Path

from trama_engine import epanet, geojson, writer

READERS = {".geojson": geojson.read, ".json": geojson.read, ".inp": epanet.read}
EXPORTERS = {"geojson": geojson.export, "epanet": epanet.export}


def compile_file(source: Path, destination: Path) -> None:
    """Compile a source network file into a TRAMA container, choosing the reader by suffix."""
    read = READERS.get(source.suffix.lower())
    if read is None:
        raise ValueError(f"unsupported input format '{source.suffix}'; expected one of {', '.join(sorted(READERS))}")
    writer.write(read(source), destination)


def export_file(source: Path, destination: Path, target: str) -> tuple[Path, ...]:
    """Export a TRAMA container to another format, returning the paths written."""
    export = EXPORTERS.get(target)
    if export is None:
        raise ValueError(f"unsupported export target '{target}'")
    return export(source, destination)
