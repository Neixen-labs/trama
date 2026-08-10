# SPDX-License-Identifier: LicenseRef-BSL-1.1
import shutil
import struct
from pathlib import Path

import pytest

from trama_engine import epanet, writer
from trama_engine.compiler import compile_file, export_file
from trama_engine.model import Source
from trama_engine.reader import read_network

_SOURCE = Path(__file__).parent / "data" / "network.inp"


def _named(network) -> dict[str, dict]:
    return {entity.properties["name"]: entity.properties for entity in [*network.nodes, *network.edges]}


def test_read_maps_every_node_and_link_kind(tmp_path: Path) -> None:
    network = epanet.read(_SOURCE)

    assert len(network.nodes) == 4
    assert len(network.edges) == 5
    named = _named(network)
    assert {name: properties["type"] for name, properties in named.items()} == {
        "J1": "junction",
        "J2": "junction",
        "R1": "reservoir",
        "T1": "tank",
        "P1": "pipe",
        "P2": "pipe",
        "P3": "pipe",
        "PU1": "pump",
        "V1": "valve",
    }
    assert named["J1"]["elevation"] == 100.0
    assert named["J1"]["demand_pattern"] == "P1"
    # An omitted trailing field stays absent rather than becoming a zero.
    assert "demand_pattern" not in named["J2"]
    assert named["P1"]["roughness"] == 100.0
    assert named["V1"]["valve_type"] == "PRV"
    # A pump's parameters are a keyword list, so they keep the rest of the line.
    assert named["PU1"]["parameters"] == "HEAD C1"


def test_read_keeps_link_vertices_and_exact_node_coordinates() -> None:
    network = epanet.read(_SOURCE)

    named = {edge.properties["name"]: edge for edge in network.edges}
    assert len(named["P2"].points) == 3
    assert len(named["P1"].points) == 2
    junction = next(node for node in network.nodes if node.properties["name"] == "J1")
    assert (junction.properties["x"], junction.properties["y"]) == (-3.704, 40.416)


def test_read_supports_parallel_links_between_the_same_nodes() -> None:
    network = epanet.read(_SOURCE)

    parallel = [edge for edge in network.edges if edge.properties["name"] in {"P2", "PU1"}]
    assert len({(edge.source, edge.target) for edge in parallel}) == 1
    assert len({edge.id for edge in parallel}) == 2


def test_round_trip_through_trama_preserves_the_network(tmp_path: Path) -> None:
    first = tmp_path / "network.trama"
    compile_file(_SOURCE, first)
    (exported,) = export_file(first, tmp_path / "again.inp", "epanet")
    second = tmp_path / "again.trama"
    compile_file(exported, second)

    before = read_network(first)
    after = read_network(second)
    assert [node.id for node in before.nodes] == [node.id for node in after.nodes]
    assert [edge.id for edge in before.edges] == [edge.id for edge in after.edges]
    assert [(edge.source, edge.target) for edge in before.edges] == [(edge.source, edge.target) for edge in after.edges]
    # Node coordinates survive exactly, because they travel as properties rather than as geometry.
    assert [node.properties for node in before.nodes] == [node.properties for node in after.nodes]
    assert [edge.properties for edge in before.edges] == [edge.properties for edge in after.edges]


def test_exported_inp_is_byte_stable_across_a_second_round_trip(tmp_path: Path) -> None:
    first = tmp_path / "network.trama"
    compile_file(_SOURCE, first)
    (once,) = export_file(first, tmp_path / "once.inp", "epanet")
    second = tmp_path / "again.trama"
    compile_file(once, second)
    (twice,) = export_file(second, tmp_path / "twice.inp", "epanet")

    assert once.read_text() == twice.read_text()


def test_read_keeps_unmodelled_sections_as_source_material() -> None:
    network = epanet.read(_SOURCE)

    (residual,) = network.sources
    assert residual.format == "epanet-inp"
    # The file name is not stored: it is not network data.
    assert residual.name == ""
    text = residual.content.decode()
    assert "[PATTERNS]" in text and "0.8\t0.9" in text
    assert "[CONTROLS]" in text and "[OPTIONS]" in text and "[TIMES]" in text
    assert "TRAMA compiler test network" in text
    # Sections the graph represents are not duplicated into the source document.
    assert "[JUNCTIONS]" not in text and "[COORDINATES]" not in text


def test_export_replays_the_simulation_sections(tmp_path: Path) -> None:
    destination = tmp_path / "network.trama"
    compile_file(_SOURCE, destination)
    (exported,) = export_file(destination, tmp_path / "again.inp", "epanet")

    text = exported.read_text()
    for section in ("[PATTERNS]", "[CURVES]", "[CONTROLS]", "[OPTIONS]", "[TIMES]"):
        assert section in text
    assert "Duration\t24:00" in text
    assert "LINK P2 CLOSED AT TIME 6" in text
    assert "Sector" not in text and "TRAMA compiler test network" in text
    # The graph still comes from the container, not from the stored text.
    assert text.count("[JUNCTIONS]") == 1


def test_source_section_is_optional_for_readers(tmp_path: Path) -> None:
    destination = tmp_path / "network.trama"
    compile_file(_SOURCE, destination)

    data = destination.read_bytes()
    count = struct.unpack_from("<I", data, 0x20)[0]
    records = {
        data[64 + index * 64 : 64 + index * 64 + 4]: struct.unpack_from("<I", data, 64 + index * 64 + 4)[0]
        for index in range(count)
    }
    assert records[b"SRCE"] & 1 == 0, "SRCE must not be marked required"
    assert all(flags & 1 for kind, flags in records.items() if kind != b"SRCE")
    # An addition that old readers may skip keeps the minimum reader version where it was.
    assert struct.unpack_from("<HHH", data, 8) == (0, 2, 0)
    assert struct.unpack_from("<HHH", data, 14) == (0, 1, 0)


def test_source_material_costs_little_next_to_the_graph(tmp_path: Path) -> None:
    """The reader stores the residue, not the whole file, so the container stays compact."""
    with_sources = tmp_path / "network.trama"
    compile_file(_SOURCE, with_sources)
    stripped = epanet.read(_SOURCE)._replace(sources=[])
    without_sources = tmp_path / "bare.trama"
    writer.write(stripped, without_sources)

    overhead = with_sources.stat().st_size - without_sources.stat().st_size
    assert 0 < overhead < _SOURCE.stat().st_size


def test_compile_rejects_coordinates_outside_wgs84(tmp_path: Path) -> None:
    source = tmp_path / "local.inp"
    source.write_text(
        "[JUNCTIONS]\n J1\t100\n J2\t95\n[PIPES]\n P1\tJ1\tJ2\t100\t300\t100\n"
        "[COORDINATES]\n J1\t20\t70\n J2\t2000\t7000\n[END]\n"
    )

    with pytest.raises(ValueError, match="outside WGS 84"):
        compile_file(source, tmp_path / "local.trama")


def test_compile_rejects_a_link_with_an_undeclared_node(tmp_path: Path) -> None:
    source = tmp_path / "broken.inp"
    source.write_text(
        "[JUNCTIONS]\n J1\t100\n[PIPES]\n P1\tJ1\tJ9\t100\t300\t100\n"
        "[COORDINATES]\n J1\t-3.7\t40.4\n[END]\n"
    )

    with pytest.raises(ValueError, match="references undeclared node 'J9'"):
        compile_file(source, tmp_path / "broken.trama")


def test_compile_rejects_an_inp_without_coordinates(tmp_path: Path) -> None:
    source = tmp_path / "flat.inp"
    shutil.copyfile(_SOURCE, source)
    source.write_text(source.read_text().split("[COORDINATES]")[0])

    with pytest.raises(ValueError, match="no \\[COORDINATES\\] section"):
        compile_file(source, tmp_path / "flat.trama")


def test_compile_rejects_an_unsupported_input_format(tmp_path: Path) -> None:
    source = tmp_path / "network.csv"
    source.write_text("id,x,y\n")

    with pytest.raises(ValueError, match="unsupported input format"):
        compile_file(source, tmp_path / "network.trama")


def test_export_ignores_graph_sections_smuggled_into_source_material(tmp_path: Path) -> None:
    """The graph is authoritative: stored text may not reintroduce a section it represents."""
    tampered = epanet.read(_SOURCE)._replace(
        sources=[Source("epanet-inp", "", b"[TITLE]\nkept\n\n[JUNCTIONS]\n FAKE\t1\t2\n")]
    )
    destination = tmp_path / "tampered.trama"
    writer.write(tampered, destination)

    (exported,) = export_file(destination, tmp_path / "out.inp", "epanet")

    text = exported.read_text()
    assert "kept" in text
    assert "FAKE" not in text
    assert text.count("[JUNCTIONS]") == 1
