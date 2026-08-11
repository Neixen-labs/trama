# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Runs EPANET over a container and returns the state deltas the engine consumes.

The container is turned back into a `.inp` and handed to the toolkit, which is the point of
the round trip being defined by simulation: if the export is faithful, the solver is solving
the user's network and not an approximation of it.
"""

from __future__ import annotations

import struct
import tempfile
from dataclasses import dataclass
from pathlib import Path

from epanet import toolkit as en
from trama_engine.compiler import read_sections

from trama_epanet.exporter import entity_ids, export_inp

DELTA = struct.Struct("<QHff")
# Coordinates never reach the hydraulics, so the projection used to rebuild the .inp is free.
# Web Mercator is the one the geometry is already stored in.
WORKING_CRS = "EPSG:3857"


@dataclass(frozen=True)
class Parameters:
    pressure_channel: str = "pressure"
    flow_channel: str = "flow"


class InvalidInput(ValueError):
    """The container or the parameters do not satisfy the solver's declared inputs."""


def solve(container: bytes, parameters: Parameters, t0_seconds: float, t1_seconds: float) -> bytes:
    """Packed deltas for every node pressure and link flow reported within [t0, t1]."""
    if t1_seconds < t0_seconds:
        raise InvalidInput("t1_seconds must not precede t0_seconds")
    declarations = next((payload for kind, _key, payload in read_sections(container) if kind == b"STCH"), None)
    if declarations is None:
        raise InvalidInput("container is missing an STCH section")
    pressure = _declared(declarations, parameters.pressure_channel, kind=1)
    flow = _declared(declarations, parameters.flow_channel, kind=2)

    with tempfile.TemporaryDirectory() as directory:
        workspace = Path(directory)
        source = workspace / "network.trama"
        source.write_bytes(container)
        try:
            nodes, links = entity_ids(source)
        except (KeyError, ValueError) as error:
            raise InvalidInput(f"container was not compiled from an EPANET network: {error}") from error
        network = workspace / "network.inp"
        export_inp(source, network, WORKING_CRS)
        return _simulate(network, workspace / "report.rpt", nodes, links, pressure, flow, t0_seconds, t1_seconds)


def _simulate(
    network: Path,
    report: Path,
    nodes: dict[str, int],
    links: dict[str, int],
    pressure: int,
    flow: int,
    t0_seconds: float,
    t1_seconds: float,
) -> bytes:
    project = en.createproject()
    records = bytearray()
    try:
        en.open(project, str(network), str(report), "")
        en.openH(project)
        en.initH(project, en.SAVE)
        node_count = en.getcount(project, en.NODECOUNT)
        link_count = en.getcount(project, en.LINKCOUNT)
        while True:
            now = float(en.runH(project))
            if t0_seconds <= now <= t1_seconds:
                for index in range(1, node_count + 1):
                    identity = nodes.get(en.getnodeid(project, index))
                    if identity is not None:
                        records += DELTA.pack(identity, pressure, now, en.getnodevalue(project, index, en.PRESSURE))
                for index in range(1, link_count + 1):
                    identity = links.get(en.getlinkid(project, index))
                    if identity is not None:
                        records += DELTA.pack(identity, flow, now, en.getlinkvalue(project, index, en.FLOW))
            if en.nextH(project) == 0:
                break
        en.closeH(project)
    except Exception as error:  # the toolkit raises its own error type for a bad network
        raise InvalidInput(f"EPANET refused the network: {error}") from error
    finally:
        en.close(project)
        en.deleteproject(project)
    return bytes(records)


def _declared(payload: bytes, name: str, kind: int) -> int:
    """Resolve a channel name to its declared id, refusing one the file never promised."""
    count, strings_offset, records_offset = struct.unpack_from("<3I", payload)
    strings = []
    at = strings_offset + 4
    for _index in range(struct.unpack_from("<I", payload, strings_offset)[0]):
        length = struct.unpack_from("<I", payload, at)[0]
        strings.append(payload[at + 4 : at + 4 + length].decode())
        at += 4 + length
    for index in range(count):
        channel_id, entity_kind, _version, name_id, _unit_id, _low, _high, _flags = struct.unpack_from(
            "<HBBIIffI", payload, records_offset + index * 24
        )
        if strings[name_id] == name and entity_kind == kind:
            return int(channel_id)
    raise InvalidInput(f"the container declares no {'node' if kind == 1 else 'edge'} channel named {name!r}")
