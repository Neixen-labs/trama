# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Rebuilds a pandapower network from a container and runs the load flow over it.

The container is the whole input. The three tables the importer expressed come back out of
``GRPH`` and ``PROP``; everything else — loads, generators, the external grid, switches, standard
types — comes out of the ``XTRA`` record, which also carries the column order to put the rows
back in. Nothing here reads a file the caller did not send.
"""

from __future__ import annotations

import json
import struct
from dataclasses import dataclass
from typing import Any

import pandapower

from .container import Container

OWNER = "power"
#: SPEC 6: (entity_id: u64, channel_id: u16, t: f32, value: f32), little-endian, no padding.
DELTA = struct.Struct("<QHff")
VOLTAGE_CHANNEL = "voltage"
LOADING_CHANNEL = "loading"


class Unsolvable(Exception):
    """The container is not a network this solver can run."""


@dataclass(frozen=True)
class Rebuilt:
    net: Any
    #: pandapower bus index to the container's stable node id.
    bus_ids: dict[int, int]
    #: ``("line" | "trafo", pandapower index)`` to the container's stable edge id.
    element_ids: dict[tuple[str, int], int]


def rebuild(container: Container) -> Rebuilt:
    """The network as pandapower knows it, plus the identities the deltas must be written against."""
    extras = [extra for extra in container.extras() if extra.owner == OWNER]
    if not extras:
        raise Unsolvable("this container was not compiled from a pandapower network")
    document = json.loads(extras[0].payload.decode("utf-8"))

    graph = container.graph()
    node_properties, edge_properties = container.properties()
    bus_ids: dict[int, int] = {}
    element_ids: dict[tuple[str, int], int] = {}

    rows: dict[str, list[tuple[int, list[Any]]]] = {"bus": [], "line": [], "trafo": []}
    for position, entity_id in enumerate(graph.node_ids):
        index = _column(node_properties, "power:index", position)
        bus_ids[index] = entity_id
        rows["bus"].append((index, _row(document, "bus", node_properties, position)))
    for position, entity_id in enumerate(graph.edge_ids):
        kind = _column(edge_properties, "power:kind", position)
        if kind not in ("line", "trafo"):
            raise Unsolvable(f"edge {entity_id} is a '{kind}', which this solver does not know")
        index = _column(edge_properties, "power:index", position)
        element_ids[(kind, index)] = entity_id
        rows[kind].append((index, _row(document, kind, edge_properties, position)))

    for table, built in rows.items():
        built.sort(key=lambda pair: pair[0])
        frame = json.loads(document["_object"][table]["_object"])
        frame["index"] = [index for index, _values in built]
        frame["data"] = [values for _index, values in built]
        document["_object"][table]["_object"] = json.dumps(frame)

    return Rebuilt(net=pandapower.from_json_string(json.dumps(document)), bus_ids=bus_ids, element_ids=element_ids)


def _row(document: dict[str, Any], table: str, properties: dict[str, list[Any]], position: int) -> list[Any]:
    """One table row, in the column order the container kept for exactly this."""
    columns = json.loads(document["_object"][table]["_object"])["columns"]
    return [_column(properties, f"power:{column}", position, required=False) for column in columns]


def _column(properties: dict[str, list[Any]], key: str, position: int, required: bool = True) -> Any:
    values = properties.get(key)
    if values is None:
        if required:
            raise Unsolvable(f"the container declares no '{key}' property")
        return None
    return values[position]


def solve(container: Container, t0_seconds: float, t1_seconds: float, params: dict[str, Any]) -> bytes:
    """Packed deltas for every bus voltage and every line and transformer loading.

    A load flow is one instant, so by default this writes one: the network as it stands, at
    ``t0``. ``load_scaling`` turns that into a series — one run per multiplier, spread evenly
    across the interval — because the daily curve is the caller's to supply, not this solver's
    to invent. Every run is a real load flow, not an interpolation between two.
    """
    if t1_seconds < t0_seconds:
        raise Unsolvable("t1_seconds must not precede t0_seconds")
    scaling = params.get("load_scaling") or [1.0]
    if not all(isinstance(factor, (int, float)) and factor >= 0 for factor in scaling):
        raise Unsolvable("load_scaling holds non-negative numbers")

    rebuilt = rebuild(container)
    declared = {channel.name: channel for channel in container.channels()}
    voltage = declared.get(params.get("voltage_channel", VOLTAGE_CHANNEL))
    loading = declared.get(params.get("loading_channel", LOADING_CHANNEL))
    if voltage is None and loading is None:
        raise Unsolvable("this container declares neither a voltage nor a loading channel")

    baseline = rebuilt.net.load.scaling.copy()
    records = []
    for step, factor in enumerate(scaling):
        moment = t0_seconds if len(scaling) == 1 else t0_seconds + (t1_seconds - t0_seconds) * step / (len(scaling) - 1)
        rebuilt.net.load.scaling = baseline * factor
        try:
            pandapower.runpp(rebuilt.net)
        except Exception as failure:  # pandapower raises its own hierarchy for non-convergence
            raise Unsolvable(f"the load flow did not converge at t={moment:g}s: {failure}") from failure
        if voltage is not None:
            for index, value in rebuilt.net.res_bus.vm_pu.items():
                records.append((rebuilt.bus_ids[index], voltage.channel_id, moment, float(value)))
        if loading is not None:
            for table, kind in (("res_line", "line"), ("res_trafo", "trafo")):
                for index, value in getattr(rebuilt.net, table).loading_percent.items():
                    records.append((rebuilt.element_ids[(kind, index)], loading.channel_id, moment, float(value)))

    # SPEC 7 of the contract: t, then channel, then unsigned entity id.
    records.sort(key=lambda record: (record[2], record[1], record[0]))
    return b"".join(DELTA.pack(*record) for record in records)
