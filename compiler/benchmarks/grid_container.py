# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Writes a grid container of a given size, for the engine's frame benchmark.

A 100k-segment container is 4.1 MB. That is generated on demand rather than committed: it
would be the largest file in the repository by an order of magnitude and it is a pure
function of `side`.

    uv run python benchmarks/grid_container.py --side 224 --out /tmp/bench.trama
"""

from __future__ import annotations

import argparse
import tempfile
import time
from pathlib import Path

from synthetic_grid import write_grid

from trama_engine.compiler import compile_geojson

CHANNELS = [{"name": "flow", "entity_kind": "edge", "unit": "1", "min": -50, "max": 50}]


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--side", type=int, default=224, help="nodes per side; 224 is about 100k edges")
    parser.add_argument("--out", type=Path, required=True)
    arguments = parser.parse_args()

    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "grid.geojson"
        edges = write_grid(source, arguments.side)
        started = time.perf_counter()
        compile_geojson(source, arguments.out, CHANNELS)
        elapsed = time.perf_counter() - started
    print(f"{edges} edges -> {arguments.out} ({arguments.out.stat().st_size / 1e6:.1f} MB, {elapsed:.1f} s)")


if __name__ == "__main__":
    main()
