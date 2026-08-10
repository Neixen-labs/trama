# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""GeoJSON in and out, per `docs/SPEC.md` section 8."""

from __future__ import annotations

import json
from pathlib import Path

from trama_engine import container
from trama_engine.model import Edge, Network, Node, network, stable_id
from trama_engine.reader import read_network

_COORDINATE_DIGITS = 7


def read(source: Path) -> Network:
    """Read LineString features, deriving one node per distinct snapped endpoint."""
    document = json.loads(source.read_text())
    nodes: dict[int, Node] = {}
    edges = []
    for feature in document.get("features") or []:
        geometry = feature.get("geometry") or {}
        if geometry.get("type") != "LineString":
            raise ValueError("v0 GeoJSON input accepts LineString features only")
        coordinates = geometry.get("coordinates") or []
        if len(coordinates) < 2:
            raise ValueError("LineString requires at least two coordinates")
        keys = [_node_key(coordinate) for coordinate in coordinates]
        points = [container.web_mercator(*coordinate[:2]) for coordinate in coordinates]
        properties = {key: value for key, value in (feature.get("properties") or {}).items() if value is not None}

        if "_trama_id" in properties:
            # An exported file carries its own identity back in; see SPEC section 8.
            edge_id = _exported_id(properties.pop("_trama_id"))
        elif feature.get("id") is not None:
            edge_id = stable_id(f"edge:{feature['id']}")
        else:
            # An id-less feature has no source identity, so derive one from its snapped geometry:
            # that keeps IDs stable when features are reordered in the source document.
            edge_id = stable_id("edge:geometry:" + ";".join(keys))

        endpoints = []
        for key, point in ((keys[0], points[0]), (keys[-1], points[-1])):
            node_id = stable_id(f"node:{key}")
            nodes.setdefault(node_id, Node(node_id, point, {}))
            endpoints.append(node_id)
        edges.append(Edge(edge_id, endpoints[0], endpoints[1], points, properties))

    if not edges:
        raise ValueError("GeoJSON contains no LineString features")
    return network(list(nodes.values()), edges)


def export(source: Path, destination: Path) -> tuple[Path, Path]:
    """Write `<destination>.nodes.geojson` and `<destination>.edges.geojson`.

    Coordinates come back from tile-quantized geometry, so they land within one
    quantization step of the source, not on it. Exact values belong in properties.
    """
    result = read_network(source)
    stem = destination.with_suffix("") if destination.suffix in {".geojson", ".json"} else destination
    nodes_path = stem.with_name(f"{stem.name}.nodes.geojson")
    edges_path = stem.with_name(f"{stem.name}.edges.geojson")

    _write(
        nodes_path,
        [
            _feature({"type": "Point", "coordinates": _position(node.point)}, node.id, node.properties)
            for node in result.nodes
        ],
    )
    _write(
        edges_path,
        [
            _feature(
                {"type": "LineString", "coordinates": [_position(point) for point in edge.points]},
                edge.id,
                edge.properties,
            )
            for edge in result.edges
        ],
    )
    return nodes_path, edges_path


def _feature(geometry: dict, entity_id: int, properties: dict[str, object]) -> dict:
    return {"type": "Feature", "geometry": geometry, "properties": {"_trama_id": str(entity_id), **properties}}


def _position(point: tuple[float, float]) -> list[float]:
    return [round(value, _COORDINATE_DIGITS) for value in container.wgs84(*point)]


def _write(path: Path, features: list[dict]) -> None:
    path.write_text(json.dumps({"type": "FeatureCollection", "features": features}, separators=(",", ":")))


def _exported_id(value: object) -> int:
    """Take back an ID this compiler wrote. A corrupt one is an error, never a silent renumber."""
    if not isinstance(value, str) or not value.isdigit() or not 0 <= int(value) < 2**64:
        raise ValueError(f"_trama_id must be a decimal u64 string, got {value!r}")
    return int(value)


def _node_key(coordinate: list[float]) -> str:
    """Snap an endpoint to a 1e-7 degree grid, roughly one centimetre.

    ponytail: exact key match, no spatial index. Add a tolerance flag when real
    sources arrive with endpoints that miss each other by more than a centimetre.
    """
    return ",".join(f"{round(float(value), 7) + 0.0:.7f}" for value in coordinate[:2])
