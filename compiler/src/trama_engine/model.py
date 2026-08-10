# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The domain-agnostic network every input format is read into and every exporter reads back.

The core knows nodes, edges, typed properties, and geometry. It never knows what a
property means.
"""

from __future__ import annotations

import hashlib
from collections.abc import Sequence
from typing import NamedTuple


class Node(NamedTuple):
    id: int
    point: tuple[float, float]
    properties: dict[str, object]


class Edge(NamedTuple):
    id: int
    source: int
    target: int
    points: list[tuple[float, float]]
    properties: dict[str, object]


class Source(NamedTuple):
    """Verbatim bytes from an input file that the container does not otherwise model."""

    format: str
    name: str
    content: bytes


class Network(NamedTuple):
    nodes: list[Node]
    edges: list[Edge]
    sources: Sequence[Source] = ()


def network(nodes: list[Node], edges: list[Edge], sources: list[Source] | None = None) -> Network:
    """Sort both entity arrays by stable ID, as the format requires, and reject a broken graph."""
    by_id = {}
    for node in nodes:
        if node.id in by_id:
            raise ValueError(f"duplicate node identity: {_label(node)}")
        by_id[node.id] = node
    seen = set()
    for edge in edges:
        if edge.id in seen:
            raise ValueError(f"duplicate edge identity: {_label(edge)}")
        seen.add(edge.id)
        for endpoint in (edge.source, edge.target):
            if endpoint not in by_id:
                raise ValueError(f"edge {_label(edge)} references an unknown node")
        if len(edge.points) < 2:
            raise ValueError(f"edge {_label(edge)} needs at least two vertices")
    if not edges:
        raise ValueError("network contains no edges")
    return Network(sorted(nodes), sorted(edges), sorted(sources or []))


def stable_id(value: str) -> int:
    """Derive a u64 identity from a source identity string."""
    return int.from_bytes(hashlib.sha256(value.encode()).digest()[:8], "little")


def _label(entity: Node | Edge) -> str:
    return str(entity.properties.get("name", entity.id))
