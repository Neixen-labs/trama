# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Export a TRAMA file back to GeoJSON, per `docs/SPEC.md` section 8."""

from __future__ import annotations

import json
from pathlib import Path

from trama_engine import container
from trama_engine.reader import read_network

_COORDINATE_DIGITS = 7


def export_geojson(source: Path, destination: Path) -> tuple[Path, Path]:
    """Write `<destination>.nodes.geojson` and `<destination>.edges.geojson`.

    Coordinates come back from tile-quantized geometry, so they land within one
    quantization step of the source, not on it. Exact values belong in properties.
    """
    network = read_network(source)
    stem = destination.with_suffix("") if destination.suffix in {".geojson", ".json"} else destination
    nodes_path = stem.with_name(f"{stem.name}.nodes.geojson")
    edges_path = stem.with_name(f"{stem.name}.edges.geojson")

    nodes = [
        _feature({"type": "Point", "coordinates": _position(point)}, node_id, {})
        for node_id, point in zip(network.node_ids, network.node_points)
    ]
    edges = [
        _feature(
            {"type": "LineString", "coordinates": [_position(point) for point in edge.points]},
            edge.id,
            edge.properties,
        )
        for edge in network.edges
    ]
    _write(nodes_path, nodes)
    _write(edges_path, edges)
    return nodes_path, edges_path


def _feature(geometry: dict, entity_id: int, properties: dict[str, object]) -> dict:
    return {
        "type": "Feature",
        "geometry": geometry,
        "properties": {"_trama_id": str(entity_id), **properties},
    }


def _position(point: tuple[float, float]) -> list[float]:
    return [round(value, _COORDINATE_DIGITS) for value in container.wgs84(*point)]


def _write(path: Path, features: list[dict]) -> None:
    path.write_text(json.dumps({"type": "FeatureCollection", "features": features}, separators=(",", ":")))
