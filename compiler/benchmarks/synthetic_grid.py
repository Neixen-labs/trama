# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Measure the phase 3 criteria: a ~50k-edge network in under 30 s, under 20% of its GeoJSON.

Deliberately not a test. It takes seconds and its numbers depend on the machine, so a CI
threshold would only produce flakes. Run it when touching anything on the compile path:

    uv run python benchmarks/synthetic_grid.py
"""

from __future__ import annotations

import json
import struct
import tempfile
import time
from pathlib import Path

from trama_engine.compiler import compile_geojson

SPACING_DEGREES = 0.0006  # about 50 m at this latitude
ORIGIN = (-3.75, 40.35)


def write_grid(destination: Path, side: int) -> int:
    """A side x side node grid wired horizontally and vertically, like a distribution network."""
    features = []
    for row in range(side):
        for column in range(side):
            longitude = ORIGIN[0] + column * SPACING_DEGREES
            latitude = ORIGIN[1] + row * SPACING_DEGREES
            if column + 1 < side:
                features.append(_edge(f"h{row}-{column}", (longitude, latitude), (longitude + SPACING_DEGREES, latitude), row, column))
            if row + 1 < side:
                features.append(_edge(f"v{row}-{column}", (longitude, latitude), (longitude, latitude + SPACING_DEGREES), row, column))
    destination.write_text(json.dumps({"type": "FeatureCollection", "features": features}))
    return len(features)


def _edge(identifier: str, start: tuple[float, float], end: tuple[float, float], row: int, column: int) -> dict:
    return {
        "type": "Feature",
        "id": identifier,
        "properties": {"label": f"pipe-{row}-{column}", "diameter": 100 + (column % 7) * 25, "loss": 0.5 + (row % 5) / 10},
        "geometry": {"type": "LineString", "coordinates": [list(start), list(end)]},
    }


def main(side: int = 158) -> None:
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "grid.geojson"
        destination = Path(directory) / "grid.trama"
        edges = write_grid(source, side)

        started = time.perf_counter()
        compile_geojson(source, destination)
        elapsed = time.perf_counter() - started

        trama_bytes = destination.stat().st_size
        print(f"{edges} edges, {source.stat().st_size / 1e6:.1f} MB source GeoJSON")
        print(f"compile   {elapsed:6.1f} s   {'PASS' if elapsed < 30 else 'FAIL'} (criterion < 30 s)")

        # The size criterion is "< 20% of the equivalent GeoJSON", and equivalence matters: the
        # source here omits nodes and rounds coordinates, so it is the smaller, harsher reference.
        # The export is the true equivalent — same entities, same IDs, full precision.
        for label, reference in _references(destination, Path(directory), source):
            share = trama_bytes / reference * 100
            verdict = "PASS" if share < 20 else "FAIL"
            print(f"size      {share:6.1f} %   {verdict}  vs {label} ({reference / 1e6:.1f} MB)")
        _print_sections(destination)


def _references(container: Path, directory: Path, source: Path) -> list[tuple[str, int]]:
    from trama_engine.exporter import export_geojson

    exported = directory / "exported"
    export_geojson(container, exported)
    compact = sum(
        len(json.dumps(json.loads(path.read_text()), separators=(",", ":"))) for path in exported.glob("*.geojson")
    )
    return [("equivalent export, compact", compact), ("hand-written source", source.stat().st_size)]


def _print_sections(container: Path) -> None:
    data = container.read_bytes()
    section_count = struct.unpack_from("<I", data, 0x20)[0]
    totals: dict[str, list[int]] = {}
    for index in range(section_count):
        record = 64 + index * 64
        kind = data[record : record + 4].decode()
        stored = struct.unpack_from("<Q", data, record + 28)[0]
        entry = totals.setdefault(kind, [0, 0])
        entry[0] += 1
        entry[1] += stored
    print("\nsection  count      stored   share")
    for kind, (count, stored) in sorted(totals.items(), key=lambda item: -item[1][1]):
        print(f"{kind:7} {count:6} {stored:11} {stored / len(data) * 100:6.1f}%")


if __name__ == "__main__":
    main()
