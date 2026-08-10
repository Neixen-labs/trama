# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Regenerates `fixtures/demo-grid.trama`, the network the demo page draws.

The container is committed so `npm run demo` works without Python, but a committed binary
with no way to rebuild it rots. Run this after any change to the format or the compiler:

    uv run python benchmarks/demo_grid.py
"""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from synthetic_grid import write_grid

from trama_engine.compiler import compile_geojson, validate_container

# 40x40 nodes at ~150 m spans: 3,120 edges over 20 tiles, enough to show tiles meeting
# without being a file anyone minds cloning.
SIDE = 40
SPACING_DEGREES = 0.0018
ORIGIN = (-3.72, 40.39)

# Declared so the demo has channels to scrub once a solver writes to them. The file never
# holds samples: STCH is the contract, not the data.
CHANNELS = [
    {"name": "pressure", "entity_kind": "node", "unit": "m", "min": 0, "max": 80},
    {"name": "flow", "entity_kind": "edge", "unit": "l/s", "min": -50, "max": 50},
]


def main() -> None:
    destination = Path(__file__).resolve().parents[2] / "fixtures" / "demo-grid.trama"
    with tempfile.TemporaryDirectory() as directory:
        source = Path(directory) / "demo-grid.geojson"
        edges = write_grid(source, SIDE, SPACING_DEGREES, ORIGIN)
        compile_geojson(source, destination, CHANNELS)
    validate_container(destination)
    print(f"{edges} edges -> {destination} ({destination.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
