# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""`.inp` -> `.trama` -> `.inp`, verified the way SPEC 9 defines it: by simulation.

Byte equality is not the criterion and could not be met — comments and field spacing are not
information about the network. What must survive is every node pressure and link flow at
every reported timestep.
"""

from pathlib import Path

import pytest
from epanet import toolkit as en
from trama_engine.compiler import compile_features, read_sections

from trama_epanet import inp
from trama_epanet.exporter import export_inp
from trama_epanet.importer import MEDIA_TYPE, OWNER, EpanetImporter

NETWORKS = Path(__file__).parent / "networks"
# Net1 and Net3 place their nodes on a small unnamed grid. Read as metres they make a network
# about 80 m across; read as degrees the same numbers would stretch each pipe over hundreds of
# kilometres and across hundreds of tiles, which is a fine thing to test but a poor default.
CRS = "EPSG:3857"


def _results(source: Path, report: Path) -> dict[tuple[str, str, int], float]:
    """Every node pressure and link flow, keyed by entity name and simulation time."""
    project = en.createproject()
    en.open(project, str(source), str(report), "")
    en.openH(project)
    en.initH(project, en.SAVE)
    nodes = en.getcount(project, en.NODECOUNT)
    links = en.getcount(project, en.LINKCOUNT)
    sampled: dict[tuple[str, str, int], float] = {}
    while True:
        now = en.runH(project)
        for index in range(1, nodes + 1):
            sampled[("node", en.getnodeid(project, index), now)] = en.getnodevalue(project, index, en.PRESSURE)
        for index in range(1, links + 1):
            sampled[("link", en.getlinkid(project, index), now)] = en.getlinkvalue(project, index, en.FLOW)
        if en.nextH(project) == 0:
            break
    en.closeH(project)
    en.close(project)
    en.deleteproject(project)
    return sampled


def _agree(expected: dict[tuple[str, str, int], float], actual: dict[tuple[str, str, int], float], label: str) -> None:
    """Solver tolerance, not float equality: rewriting `10530` as `10530.0` moves last bits."""
    assert actual.keys() == expected.keys()
    worst = max(abs(actual[key] - value) for key, value in expected.items())
    assert worst < 1e-3, f"{label} drifted by {worst}"


def _round_trip(name: str, tmp_path: Path) -> tuple[Path, Path]:
    source = NETWORKS / name
    container = tmp_path / "network.trama"
    imported = EpanetImporter().load(source, {"source-crs": CRS})
    compile_features(imported.features, container, extras=imported.extras)
    rebuilt = tmp_path / "rebuilt.inp"
    export_inp(container, rebuilt, CRS)
    return source, rebuilt


@pytest.mark.parametrize("network", ["Net1.inp", "Net3.inp"])
def test_the_rebuilt_network_simulates_identically(network: str, tmp_path: Path) -> None:
    source, rebuilt = _round_trip(network, tmp_path)

    _agree(_results(source, tmp_path / "source.rpt"), _results(rebuilt, tmp_path / "rebuilt.rpt"), network)


@pytest.mark.parametrize("network", ["Net1.inp", "Net3.inp"])
def test_the_container_carries_one_opaque_record_and_the_core_still_validates(network: str, tmp_path: Path) -> None:
    _source, _rebuilt = _round_trip(network, tmp_path)

    sections = read_sections((tmp_path / "network.trama").read_bytes())
    extras = [payload for kind, _key, payload in sections if kind == b"XTRA"]
    assert len(extras) == 1
    assert OWNER.encode() in extras[0] and MEDIA_TYPE.encode() in extras[0]
    # What the core cannot type went in; what it can type stayed out.
    assert b"[PATTERNS]" in extras[0]
    assert b"[JUNCTIONS]" not in extras[0] and b"[COORDINATES]" not in extras[0]


def test_an_inp_without_a_declared_crs_is_refused(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="coordinate reference system"):
        EpanetImporter().load(NETWORKS / "Net1.inp", {})


def test_a_link_crossing_tiles_comes_back_with_the_tile_boundaries_as_vertices(tmp_path: Path) -> None:
    """Pinned, not fixed: geometry is stored per tile, so a long link gains vertices.

    Read as degrees, Net1's pipes span hundreds of kilometres and cross hundreds of tiles.
    Each crossing is a real stored vertex, lying on the original segment. EPANET reads no
    coordinate, so the simulation is unaffected, but the file does grow.
    """
    imported = EpanetImporter().load(NETWORKS / "Net1.inp", {"source-crs": "EPSG:4326"})
    container = tmp_path / "spread.trama"
    compile_features(imported.features, container, extras=imported.extras)
    rebuilt = tmp_path / "spread.inp"

    export_inp(container, rebuilt, "EPSG:4326")

    source_vertices = len(inp.parse((NETWORKS / "Net1.inp").read_text()).rows("VERTICES"))
    assert source_vertices == 0
    assert len(inp.parse(rebuilt.read_text()).rows("VERTICES")) > 1000
    _agree(_results(NETWORKS / "Net1.inp", tmp_path / "a.rpt"), _results(rebuilt, tmp_path / "b.rpt"), "spread Net1")
