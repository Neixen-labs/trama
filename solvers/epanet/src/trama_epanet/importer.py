# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Reads EPANET `.inp` into the features and opaque records the compiler accepts.

The split follows `docs/EPANET_BOUNDARY.md`: the six sections that define entities become
nodes, edges, and typed properties; `[COORDINATES]` and `[VERTICES]` become geometry; every
other section travels verbatim in one `XTRA` record, because a demand pattern or a control
rule is not a value attached to an entity and SPEC 7.1 will not have it faked as one.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
from typing import Any

from pyproj import Transformer
from trama_engine.compiler import Extra
from trama_engine.importers import Import

from trama_epanet import inp

OWNER = "epanet"
MEDIA_TYPE = "text/vnd.epanet.inp-sections"
EXPRESSED = {"JUNCTIONS", "RESERVOIRS", "TANKS", "PIPES", "PUMPS", "VALVES", "COORDINATES", "VERTICES"}

# Field names per section, in file order. A name prefixed with `#` is stored as a number;
# anything else stays a string, because EPANET allows a curve or pattern id in those columns
# and one string beats a column whose type depends on the network.
NODE_FIELDS = {
    "JUNCTIONS": ["#elevation", "#demand", "pattern"],
    "RESERVOIRS": ["#head", "pattern"],
    "TANKS": ["#elevation", "#init-level", "#min-level", "#max-level", "#diameter", "#min-volume", "volume-curve", "overflow"],
}
LINK_FIELDS = {
    "PIPES": ["#length", "#diameter", "#roughness", "#minor-loss", "status"],
    "VALVES": ["#diameter", "valve-type", "setting", "#minor-loss"],
}
KINDS = {"JUNCTIONS": "junction", "RESERVOIRS": "reservoir", "TANKS": "tank", "PIPES": "pipe", "PUMPS": "pump", "VALVES": "valve"}
# `[OPTIONS] Units` sets the whole file's unit system, and EPANET reports pressure in psi for
# the US flow units and in metres for the SI ones. A channel declaration that named the wrong
# one would be a lie the file tells every solver that reads it.
US_FLOW_UNITS = ("cfs", "gpm", "mgd", "imgd", "afd")
SI_FLOW_UNITS = ("lps", "lpm", "mld", "cmh", "cmd")
FLOW_UNITS = US_FLOW_UNITS + SI_FLOW_UNITS
PRESSURE_UNITS = ("psi", "m")
DEFAULT_FLOW_UNITS = "gpm"  # EPANET's own default when [OPTIONS] says nothing


class EpanetImporter:
    """Registered under the `trama.importers` entry-point group."""

    suffixes = (".inp",)

    def load(self, source: Path, options: Mapping[str, str]) -> Import:
        crs = options.get("source-crs")
        if not crs:
            raise ValueError(
                "an EPANET .inp declares no coordinate reference system; pass -o source-crs=EPSG:xxxx"
            )
        document = inp.parse(source.read_text())
        to_wgs84 = Transformer.from_crs(crs, "EPSG:4326", always_xy=True).transform
        positions = {row[0]: (inp.number(row[1]), inp.number(row[2])) for row in document.rows("COORDINATES")}
        vertices: dict[str, list[tuple[float, float]]] = {}
        for row in document.rows("VERTICES"):
            vertices.setdefault(row[0], []).append((inp.number(row[1]), inp.number(row[2])))

        features = [
            _point(name, properties, positions, to_wgs84) for name, properties in _nodes(document)
        ]
        features += [
            _line(name, source_node, target_node, properties, positions, vertices, to_wgs84)
            for name, source_node, target_node, properties in _links(document)
        ]
        remainder = inp.serialize(document.without(EXPRESSED))
        return Import(
            features=features,
            extras=[Extra(OWNER, MEDIA_TYPE, remainder.encode())],
            channels=channels(document),
        )


def _nodes(document: inp.Document) -> list[tuple[str, dict[str, Any]]]:
    return [
        (row[0], {"epanet:kind": KINDS[name], "epanet:name": row[0], **_fields(fields, row[1:])})
        for name, fields in NODE_FIELDS.items()
        for row in document.rows(name)
    ]


def _links(document: inp.Document) -> list[tuple[str, str, str, dict[str, Any]]]:
    links = [
        (row[0], row[1], row[2], {"epanet:kind": KINDS[name], "epanet:name": row[0], **_fields(fields, row[3:])})
        for name, fields in LINK_FIELDS.items()
        for row in document.rows(name)
    ]
    # A pump's parameters are keyword-value pairs whose count and meaning vary; the line is
    # kept whole rather than guessed at, and EPANET reads back what it wrote.
    links += [
        (row[0], row[1], row[2], {"epanet:kind": "pump", "epanet:name": row[0], "epanet:parameters": " ".join(row[3:])})
        for row in document.rows("PUMPS")
    ]
    return links


def _fields(names: list[str], row: list[str]) -> dict[str, Any]:
    """Name the fields a row actually carries. A trailing field EPANET omits stays absent."""
    described = {}
    for name, value in zip(names, row):
        if not value:
            continue
        described[f"epanet:{name.lstrip('#')}"] = inp.number(value) if name.startswith("#") else value
    return described


def _point(
    name: str,
    properties: dict[str, Any],
    positions: dict[str, tuple[float, float]],
    to_wgs84: Any,
) -> dict[str, Any]:
    return {
        "type": "Feature",
        "properties": properties,
        "geometry": {"type": "Point", "coordinates": list(to_wgs84(*_position(name, positions)))},
    }


def _line(
    name: str,
    source_node: str,
    target_node: str,
    properties: dict[str, Any],
    positions: dict[str, tuple[float, float]],
    vertices: dict[str, list[tuple[float, float]]],
    to_wgs84: Any,
) -> dict[str, Any]:
    path = [_position(source_node, positions), *vertices.get(name, []), _position(target_node, positions)]
    return {
        "type": "Feature",
        "id": name,
        "properties": properties,
        "geometry": {"type": "LineString", "coordinates": [list(to_wgs84(*point)) for point in path]},
    }


def _position(name: str, positions: dict[str, tuple[float, float]]) -> tuple[float, float]:
    if name not in positions:
        raise ValueError(f"node {name!r} has no entry in [COORDINATES], so it cannot be placed on a map")
    return positions[name]


def channels(document: inp.Document) -> list[dict[str, Any]]:
    """What a container built from this file may be solved for, in the file's own units."""
    flow_units = next(
        (row[-1].lower() for row in document.rows("OPTIONS") if row[0].lower() == "units"),
        DEFAULT_FLOW_UNITS,
    )
    # An unknown keyword would become a declared unit no solver could match, and the mismatch
    # would surface at solve time rather than here, where the file that caused it is in hand.
    if flow_units not in FLOW_UNITS:
        raise ValueError(f"[OPTIONS] Units names {flow_units!r}, which is not an EPANET flow unit")
    return [
        {"name": "pressure", "entity_kind": "node", "unit": "psi" if flow_units in US_FLOW_UNITS else "m"},
        {"name": "flow", "entity_kind": "edge", "unit": flow_units},
    ]
