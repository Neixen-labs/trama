# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Rebuilds an EPANET `.inp` from a container.

The entity sections are written from typed properties and the graph; everything else is the
`XTRA` record handed back unread. Coordinates come out of quantized geometry, so they are
not the source numbers to the last digit — SPEC 9 defines this round trip by simulation
results, and EPANET's hydraulics never read a coordinate.
"""

from __future__ import annotations

import json
import struct
import tempfile
from pathlib import Path
from typing import Any

from pyproj import Transformer
from trama_engine.compiler import parse_graph, read_sections
from trama_engine.exporter import export_geojson

from trama_epanet import inp
from trama_epanet.importer import LINK_FIELDS, MEDIA_TYPE, NODE_FIELDS, OWNER

HEADERS = {
    "JUNCTIONS": "ID              \tElev        \tDemand      \tPattern",
    "RESERVOIRS": "ID              \tHead        \tPattern",
    "TANKS": "ID              \tElevation   \tInitLevel   \tMinLevel    \tMaxLevel    \tDiameter    \tMinVol      \tVolCurve",
    "PIPES": "ID              \tNode1           \tNode2           \tLength      \tDiameter    \tRoughness   \tMinorLoss   \tStatus",
    "PUMPS": "ID              \tNode1           \tNode2           \tParameters",
    "VALVES": "ID              \tNode1           \tNode2           \tDiameter    \tType\tSetting     \tMinorLoss",
    "COORDINATES": "Node            \tX-Coord         \tY-Coord",
    "VERTICES": "Link            \tX-Coord         \tY-Coord",
}


def export_inp(source: Path, destination: Path, crs: str) -> None:
    """Write a `.inp` reprojecting geometry back into `crs`, the one the import was given."""
    remainder = _remainder(source)
    nodes, edges = _features(source)
    to_source = Transformer.from_crs("EPSG:4326", crs, always_xy=True).transform

    entities = [
        inp.section(name, HEADERS[name], _node_rows(name, fields, nodes))
        for name, fields in NODE_FIELDS.items()
    ]
    entities += [inp.section("PIPES", HEADERS["PIPES"], _link_rows("pipe", LINK_FIELDS["PIPES"], edges))]
    entities += [inp.section("PUMPS", HEADERS["PUMPS"], _pump_rows(edges))]
    entities += [inp.section("VALVES", HEADERS["VALVES"], _link_rows("valve", LINK_FIELDS["VALVES"], edges))]

    geometry = [
        inp.section("COORDINATES", HEADERS["COORDINATES"], _coordinate_rows(nodes, to_source)),
        inp.section("VERTICES", HEADERS["VERTICES"], _vertex_rows(edges, to_source)),
    ]
    title = [entry for entry in remainder.sections if entry[0] == "TITLE"]
    end = [entry for entry in remainder.sections if entry[0] == "END"] or [("END", [])]
    rest = [entry for entry in remainder.sections if entry[0] not in {"TITLE", "END"}]
    destination.write_text(inp.serialize(inp.Document([*title, *entities, *rest, *geometry, *end])))


def entity_ids(source: Path) -> tuple[dict[str, int], dict[str, int]]:
    """EPANET names mapped to stable `u64` identities, for nodes and for links.

    A solver writes deltas against those identities; the names only exist to talk to EPANET.
    """
    nodes, edges = _features(source)
    return (
        {feature["properties"]["epanet:name"]: int(feature["properties"]["_trama_id"]) for feature in nodes},
        {feature["properties"]["epanet:name"]: int(feature["properties"]["_trama_id"]) for feature in edges},
    )


def _remainder(source: Path) -> inp.Document:
    """The sections the core carried without reading them."""
    for kind, _key, payload in read_sections(source.read_bytes()):
        if kind != b"XTRA":
            continue
        owner_offset, owner_bytes, media_offset, media_bytes, body_offset, body_bytes = _header(payload)
        owner = payload[owner_offset : owner_offset + owner_bytes].decode()
        media_type = payload[media_offset : media_offset + media_bytes].decode()
        if (owner, media_type) == (OWNER, MEDIA_TYPE):
            return inp.parse(payload[body_offset : body_offset + body_bytes].decode())
    raise ValueError("this container carries no EPANET sections; it was not compiled from a .inp")


def _header(payload: bytes) -> tuple[int, int, int, int, int, int]:
    return struct.unpack_from("<6I", payload)


def _features(source: Path) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    """Node and edge features, each edge told which nodes it joins."""
    graph = next(payload for kind, _key, payload in read_sections(source.read_bytes()) if kind == b"GRPH")
    node_ids, edge_records, _refs = parse_graph(graph)
    with tempfile.TemporaryDirectory() as directory:
        export_geojson(source, Path(directory))
        nodes = json.loads((Path(directory) / "nodes.geojson").read_text())["features"]
        edges = json.loads((Path(directory) / "edges.geojson").read_text())["features"]
    named = {feature["properties"]["_trama_id"]: feature for feature in nodes}
    by_id = {str(record[0]): record for record in edge_records}
    for feature in edges:
        _edge_id, source_index, target_index, *_rest = by_id[feature["properties"]["_trama_id"]]
        feature["endpoints"] = [
            named[str(node_ids[index][0])]["properties"]["epanet:name"] for index in (source_index, target_index)
        ]
    return nodes, edges


def _node_rows(name: str, fields: list[str], nodes: list[dict[str, Any]]) -> list[list[str]]:
    kind = name.rstrip("S").lower()
    return [
        [feature["properties"]["epanet:name"], *_fields(fields, feature["properties"])]
        for feature in sorted(nodes, key=lambda feature: feature["properties"].get("epanet:name", ""))
        if feature["properties"].get("epanet:kind") == kind
    ]


def _link_rows(kind: str, fields: list[str], edges: list[dict[str, Any]]) -> list[list[str]]:
    return [
        [feature["properties"]["epanet:name"], *feature["endpoints"], *_fields(fields, feature["properties"])]
        for feature in _of_kind(kind, edges)
    ]


def _pump_rows(edges: list[dict[str, Any]]) -> list[list[str]]:
    return [
        [feature["properties"]["epanet:name"], *feature["endpoints"], feature["properties"].get("epanet:parameters", "")]
        for feature in _of_kind("pump", edges)
    ]


def _of_kind(kind: str, edges: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return sorted(
        (feature for feature in edges if feature["properties"].get("epanet:kind") == kind),
        key=lambda feature: feature["properties"]["epanet:name"],
    )


def _fields(names: list[str], properties: dict[str, Any]) -> list[str]:
    """Render the fields a row carries, up to the last one present.

    A numeric field missing before a present one becomes `0`, which is what EPANET writes.
    A missing *text* field in that position has no such convention, so it raises rather than
    guess at a placeholder EPANET might read as a pattern name.
    """
    present = [properties.get(f"epanet:{name.lstrip('#')}") for name in names]
    last = max((index for index, value in enumerate(present) if value is not None), default=-1)
    rendered = []
    for name, value in zip(names[: last + 1], present[: last + 1]):
        if value is None and not name.startswith("#"):
            raise ValueError(f"{name.lstrip('#')!r} is absent but a later field is present, and EPANET has no blank for it")
        rendered.append(inp.text(0.0 if value is None else value))
    return rendered


def _coordinate_rows(nodes: list[dict[str, Any]], to_source: Any) -> list[list[str]]:
    return [
        [feature["properties"]["epanet:name"], *(f"{value:.4f}" for value in to_source(*feature["geometry"]["coordinates"]))]
        for feature in sorted(nodes, key=lambda feature: feature["properties"].get("epanet:name", ""))
    ]


def _vertex_rows(edges: list[dict[str, Any]], to_source: Any) -> list[list[str]]:
    return [
        [feature["properties"]["epanet:name"], *(f"{value:.4f}" for value in to_source(*point))]
        for feature in sorted(edges, key=lambda feature: feature["properties"]["epanet:name"])
        for point in feature["geometry"]["coordinates"][1:-1]
    ]
