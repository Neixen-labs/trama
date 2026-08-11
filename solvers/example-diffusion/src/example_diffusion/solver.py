# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""A pulse diffusing outward over a graph's own topology.

Deliberately domain-agnostic: the only thing it knows is that edges connect nodes. It exists to
put `docs/SOLVER_CONTRACT.md` under a real implementation, not to model anything.
"""

from __future__ import annotations

import math
import struct
from collections import deque
from dataclasses import dataclass

from trama_engine.compiler import parse_graph, read_sections

DELTA = struct.Struct("<QHff")


@dataclass(frozen=True)
class Parameters:
    channel: str = "flow"
    seed_node_index: int = 0
    step_seconds: float = 60.0
    speed_hops_per_step: float = 1.5
    amplitude: float = 40.0


class InvalidInput(ValueError):
    """The container or the parameters do not satisfy the solver's declared inputs."""


def solve(container: bytes, parameters: Parameters, t0_seconds: float, t1_seconds: float) -> bytes:
    """Return the packed delta stream for the closed interval [t0, t1]."""
    if t1_seconds < t0_seconds:
        raise InvalidInput("t1_seconds must not precede t0_seconds")
    sections = read_sections(container)
    graph = next((payload for kind, _key, payload in sections if kind == b"GRPH"), None)
    channels = next((payload for kind, _key, payload in sections if kind == b"STCH"), None)
    if graph is None or channels is None:
        raise InvalidInput("container is missing a GRPH or STCH section")

    channel_id, declared_min, declared_max = _declared_channel(channels, parameters.channel)
    nodes, edges, _refs = parse_graph(graph)
    if not nodes or not edges:
        raise InvalidInput("solver requires a container with nodes and edges")
    if not 0 <= parameters.seed_node_index < len(nodes):
        raise InvalidInput(f"seed_node_index {parameters.seed_node_index} names no node")

    hops = _hops_from_seed(len(nodes), edges, parameters.seed_node_index)
    steps = math.floor((t1_seconds - t0_seconds) / parameters.step_seconds) + 1
    records = []
    for step in range(steps):
        t = t0_seconds + step * parameters.step_seconds
        front = step * parameters.speed_hops_per_step
        for edge_id, source_index, target_index, *_rest in edges:
            reachable = [hops[index] for index in (source_index, target_index) if index in hops]
            if not reachable:
                continue  # a component the pulse never reaches
            distance = min(reachable)
            value = _clamp(parameters.amplitude * _pulse(distance - front), declared_min, declared_max)
            records.append((t, channel_id, edge_id, value))
    # SPEC-contract section 7: sorted by t, then channel, then unsigned entity id.
    records.sort(key=lambda record: (record[0], record[1], record[2]))
    return b"".join(DELTA.pack(edge_id, channel, t, value) for t, channel, edge_id, value in records)


def _pulse(offset: float) -> float:
    """A unit-height bump; a wave crest passing an edge rather than a step change."""
    return math.exp(-((offset / 1.5) ** 2))


def _clamp(value: float, low: float | None, high: float | None) -> float:
    if low is not None and value < low:
        return low
    if high is not None and value > high:
        return high
    return value


def _hops_from_seed(node_count: int, edges: list[tuple[int, ...]], seed: int) -> dict[int, int]:
    """Breadth-first hop count over the edge list, so the pulse follows real topology.

    A node absent from the result is in another component; that is a fact about the graph, not a
    missing value, so it is an absent key rather than a None to thread through the caller.
    """
    neighbours: list[list[int]] = [[] for _index in range(node_count)]
    for _edge_id, source_index, target_index, *_rest in edges:
        neighbours[source_index].append(target_index)
        neighbours[target_index].append(source_index)
    hops = {seed: 0}
    queue = deque([seed])
    while queue:
        current = queue.popleft()
        for neighbour in neighbours[current]:
            if neighbour not in hops:
                hops[neighbour] = hops[current] + 1
                queue.append(neighbour)
    return hops


def _declared_channel(payload: bytes, name: str) -> tuple[int, float | None, float | None]:
    """Resolve a channel by name through STCH, refusing to write one the file never declared."""
    channel_count, strings_offset, channels_offset = struct.unpack_from("<3I", payload)
    strings = _read_strings(payload, strings_offset)
    for index in range(channel_count):
        channel_id, entity_kind, _value_type, name_id, _unit_id, minimum, maximum, flags = struct.unpack_from(
            "<HBBIIffI", payload, channels_offset + index * 24
        )
        if strings[name_id] != name:
            continue
        if entity_kind != 2:
            raise InvalidInput(f"channel {name!r} applies to nodes; this solver writes edges")
        ranged = flags & 1
        return channel_id, minimum if ranged else None, maximum if ranged else None
    raise InvalidInput(f"container declares no channel named {name!r}")


def _read_strings(payload: bytes, offset: int) -> list[str]:
    count = struct.unpack_from("<I", payload, offset)[0]
    values, at = [], offset + 4
    for _index in range(count):
        length = struct.unpack_from("<I", payload, at)[0]
        values.append(payload[at + 4 : at + 4 + length].decode())
        at += 4 + length
    return values
