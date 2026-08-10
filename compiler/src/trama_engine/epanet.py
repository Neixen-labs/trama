# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""EPANET `.inp` in and out.

Domain knowledge stops at this file. Everything it produces is a node, an edge, or a
typed property with a name; the core never learns what a pipe is.
"""

from __future__ import annotations

from pathlib import Path

from trama_engine import container
from trama_engine.model import Edge, Network, Node, network, stable_id
from trama_engine.reader import read_network

# Section name, entity type label, and the property each positional field carries.
_NODE_SECTIONS = {
    "JUNCTIONS": ("junction", ["elevation", "demand", "demand_pattern"]),
    "RESERVOIRS": ("reservoir", ["head", "head_pattern"]),
    "TANKS": (
        "tank",
        ["elevation", "initial_level", "minimum_level", "maximum_level", "diameter", "minimum_volume", "volume_curve"],
    ),
}
_LINK_SECTIONS = {
    "PIPES": ("pipe", ["length", "diameter", "roughness", "minor_loss", "status"]),
    "PUMPS": ("pump", ["parameters"]),
    "VALVES": ("valve", ["diameter", "valve_type", "setting", "minor_loss"]),
}
_TEXT_PROPERTIES = {"demand_pattern", "head_pattern", "volume_curve", "status", "parameters", "valve_type"}
# A pump's parameters are a keyword list, not one field, so this property takes the rest of the line.
_TAIL_PROPERTIES = {"parameters"}


def read(source: Path) -> Network:
    """Read the topological sections of an `.inp` file into a network."""
    sections = _sections(source.read_text())
    coordinates = _coordinates(sections)

    nodes = []
    node_ids = {}
    for section, (label, fields) in _NODE_SECTIONS.items():
        for row in sections.get(section, []):
            name = row[0]
            if name not in coordinates:
                raise ValueError(f"node '{name}' has no entry in [COORDINATES]")
            node_id = stable_id(f"node:epanet:{name}")
            node_ids[name] = node_id
            x, y = coordinates[name]
            # Keep the exact source coordinates: render geometry is quantized, these are not.
            properties = {"name": name, "type": label, "x": x, "y": y, **_properties(row[1:], fields)}
            nodes.append(Node(node_id, container.web_mercator(x, y), properties))

    vertices = _vertices(sections)
    edges = []
    for section, (label, fields) in _LINK_SECTIONS.items():
        for row in sections.get(section, []):
            name, source_name, target_name = row[0], row[1], row[2]
            for endpoint in (source_name, target_name):
                if endpoint not in node_ids:
                    raise ValueError(f"link '{name}' references undeclared node '{endpoint}'")
            points = [container.web_mercator(*coordinates[source_name])]
            points += [container.web_mercator(x, y) for x, y in vertices.get(name, [])]
            points.append(container.web_mercator(*coordinates[target_name]))
            edges.append(
                Edge(
                    stable_id(f"edge:epanet:{name}"),
                    node_ids[source_name],
                    node_ids[target_name],
                    points,
                    {"name": name, "type": label, **_properties(row[3:], fields)},
                )
            )

    if not nodes:
        raise ValueError("EPANET file declares no nodes")
    return network(nodes, edges)


def export(source: Path, destination: Path) -> tuple[Path]:
    """Write the topological sections of a network back out as an `.inp` file.

    ponytail: rebuilds the graph sections only. Patterns, curves, controls, options,
    and times are not in the container, so they cannot come back; see the README.
    """
    result = read_network(source)
    path = destination if destination.suffix else destination.with_suffix(".inp")
    nodes_by_id = {node.id: node for node in result.nodes}
    by_type: dict[str, list] = {}
    for entity in [*result.nodes, *result.edges]:
        by_type.setdefault(str(entity.properties.get("type", "")), []).append(entity)
    for entities in by_type.values():
        entities.sort(key=_name)

    lines = ["[TITLE]", "Exported by trama-engine", ""]
    for section, (label, fields) in _NODE_SECTIONS.items():
        lines += [f"[{section}]", ";ID\t" + "\t".join(fields)]
        for node in by_type.get(label, []):
            lines.append("\t".join([_name(node), *_fields(node, fields)]))
        lines.append("")
    for section, (label, fields) in _LINK_SECTIONS.items():
        lines += [f"[{section}]", ";ID\tNode1\tNode2\t" + "\t".join(fields)]
        for edge in by_type.get(label, []):
            source_node = nodes_by_id[edge.source]
            target_node = nodes_by_id[edge.target]
            lines.append("\t".join([_name(edge), _name(source_node), _name(target_node), *_fields(edge, fields)]))
        lines.append("")

    lines += ["[COORDINATES]", ";Node\tX-Coord\tY-Coord"]
    for node in sorted(result.nodes, key=_name):
        x, y = _position(node)
        lines.append(f"{_name(node)}\t{_number(x)}\t{_number(y)}")
    lines += ["", "[VERTICES]", ";Link\tX-Coord\tY-Coord"]
    for edge in sorted(result.edges, key=_name):
        for point in edge.points[1:-1]:
            x, y = container.wgs84(*point)
            lines.append(f"{_name(edge)}\t{_number(x)}\t{_number(y)}")
    lines += ["", "[END]", ""]
    path.write_text("\n".join(lines))
    return (path,)


def _sections(text: str) -> dict[str, list[list[str]]]:
    """Split an `.inp` file into `{SECTION: [fields per line]}`, dropping comments."""
    sections: dict[str, list[list[str]]] = {}
    current = None
    for line in text.splitlines():
        line = line.split(";", 1)[0].strip()
        if not line:
            continue
        if line.startswith("["):
            current = line.strip("[]").strip().upper()
            sections.setdefault(current, [])
        elif current is not None:
            sections[current].append(line.split())
    return sections


def _coordinates(sections: dict[str, list[list[str]]]) -> dict[str, tuple[float, float]]:
    rows = sections.get("COORDINATES")
    if not rows:
        raise ValueError("EPANET file has no [COORDINATES] section, so it carries no geometry")
    coordinates = {}
    for row in rows:
        longitude, latitude = float(row[1]), float(row[2])
        if not -180 <= longitude <= 180 or not -90 <= latitude <= 90:
            raise ValueError(
                f"node '{row[0]}' at ({longitude}, {latitude}) is outside WGS 84; "
                "v0 needs a georeferenced .inp in degrees"
            )
        coordinates[row[0]] = (longitude, latitude)
    return coordinates


def _vertices(sections: dict[str, list[list[str]]]) -> dict[str, list[tuple[float, float]]]:
    vertices: dict[str, list[tuple[float, float]]] = {}
    for row in sections.get("VERTICES", []):
        vertices.setdefault(row[0], []).append((float(row[1]), float(row[2])))
    return vertices


def _properties(values: list[str], fields: list[str]) -> dict[str, object]:
    """Map positional fields to named properties, skipping the ones the line omits."""
    properties: dict[str, object] = {}
    for index, (field, value) in enumerate(zip(fields, values)):
        if field in _TAIL_PROPERTIES:
            properties[field] = " ".join(values[index:])
            break
        properties[field] = value if field in _TEXT_PROPERTIES else float(value)
    return properties


def _fields(entity: Node | Edge, fields: list[str]) -> list[str]:
    """Emit fields positionally up to the last one present, since EPANET reads them by position."""
    present = [index for index, field in enumerate(fields) if field in entity.properties]
    if not present:
        return []
    return [_number(entity.properties.get(field, 0.0)) for field in fields[: present[-1] + 1]]


def _position(node: Node) -> tuple[float, float]:
    """Prefer the exact source coordinates over the quantized render geometry."""
    if isinstance(node.properties.get("x"), float) and isinstance(node.properties.get("y"), float):
        return node.properties["x"], node.properties["y"]
    return container.wgs84(*node.point)


def _name(entity: Node | Edge) -> str:
    name = entity.properties.get("name")
    if name is None:
        raise ValueError(f"entity {entity.id} has no name property, so it cannot be written as EPANET")
    return str(name)


def _number(value: object) -> str:
    if isinstance(value, float):
        return repr(round(value, 10)) if value % 1 else str(int(value))
    return str(value)
