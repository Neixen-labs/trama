# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Node identity comes from the geometry grid, per SPEC 4.2."""

import json
import math
import struct
from pathlib import Path

from trama_engine.compiler import compile_geojson, read_sections

# The z14 tile boundary nearest the fixtures, derived from the tile scheme itself:
# world * (k / 16384 - 0.5) in Web Mercator metres, converted back to degrees.
TILE_BOUNDARY_LONGITUDE = -3.66943359375


def _compile(tmp_path: Path, *lines: list[list[float]]) -> Path:
    source = tmp_path / "network.geojson"
    source.write_text(
        json.dumps(
            {
                "type": "FeatureCollection",
                "features": [
                    {
                        "type": "Feature",
                        "id": f"edge-{index}",
                        "properties": {},
                        "geometry": {"type": "LineString", "coordinates": coordinates},
                    }
                    for index, coordinates in enumerate(lines)
                ],
            }
        )
    )
    destination = tmp_path / "network.trama"
    compile_geojson(source, destination)
    return destination


def _node_count(container: Path) -> int:
    graph = next(payload for kind, _key, payload in read_sections(container.read_bytes()) if kind == b"GRPH")
    return int(struct.unpack_from("<I", graph)[0])


def test_endpoints_differing_in_their_last_bit_are_one_node(tmp_path: Path) -> None:
    shared = [-3.704, 40.416]
    drifted = [math.nextafter(shared[0], math.inf), math.nextafter(shared[1], -math.inf)]

    container = _compile(tmp_path, [[-3.705, 40.415], shared], [drifted, [-3.703, 40.417]])

    assert _node_count(container) == 3


def test_endpoints_a_metre_apart_stay_two_nodes(tmp_path: Path) -> None:
    # A cell is about 4 cm, so a metre must survive as two nodes: the grid joins what the
    # file cannot tell apart, and nothing coarser than that.
    metre = 1 / 111_320

    container = _compile(tmp_path, [[-3.705, 40.415], [-3.704, 40.416]], [[-3.704, 40.416 + metre], [-3.703, 40.417]])

    assert _node_count(container) == 4


def test_a_node_on_a_tile_boundary_joins_across_the_boundary(tmp_path: Path) -> None:
    # Each edge quantizes this endpoint inside a different tile, one as qx = 65535 and the
    # other as qx = 0. SPEC 4.2 makes those one cell.
    meeting = [TILE_BOUNDARY_LONGITUDE, 40.416]

    container = _compile(tmp_path, [[TILE_BOUNDARY_LONGITUDE - 0.01, 40.415], meeting], [meeting, [TILE_BOUNDARY_LONGITUDE + 0.01, 40.417]])

    assert _node_count(container) == 3
