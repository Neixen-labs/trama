# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""On a platform with no EPANET wheel, only the simulation should be missing."""

import sys
from pathlib import Path

import pytest
from trama_engine.compiler import compile_features, read_sections

from trama_epanet.exporter import export_inp
from trama_epanet.importer import EpanetImporter
from trama_epanet.solver import Parameters, ToolkitUnavailable, solve, toolkit

NETWORKS = Path(__file__).parent / "networks"


@pytest.fixture
def uninstalled(monkeypatch: pytest.MonkeyPatch) -> None:
    """`None` in sys.modules is what Python raises ImportError for, without touching disk."""
    monkeypatch.setitem(sys.modules, "epanet", None)
    monkeypatch.delitem(sys.modules, "epanet.toolkit", raising=False)


def test_the_message_says_what_broke_and_what_still_works(uninstalled: None) -> None:
    with pytest.raises(ToolkitUnavailable) as raised:
        toolkit()

    assert "no Windows wheel" in str(raised.value)
    assert "--no-dev" in str(raised.value)


def test_solving_refuses_before_it_rebuilds_anything(uninstalled: None, tmp_path: Path) -> None:
    imported = EpanetImporter().load(NETWORKS / "Net1.inp", {"source-crs": "EPSG:3857"})
    container = tmp_path / "net1.trama"
    compile_features(imported.features, container, imported.channels, imported.extras)

    with pytest.raises(ToolkitUnavailable):
        solve(container.read_bytes(), Parameters(), 0.0, 3600.0)


def test_import_and_export_do_not_need_the_toolkit(uninstalled: None, tmp_path: Path) -> None:
    imported = EpanetImporter().load(NETWORKS / "Net1.inp", {"source-crs": "EPSG:3857"})
    container = tmp_path / "net1.trama"
    compile_features(imported.features, container, imported.channels, imported.extras)
    rebuilt = tmp_path / "net1.inp"

    export_inp(container, rebuilt, "EPSG:3857")

    assert "[JUNCTIONS]" in rebuilt.read_text()
    assert [kind for kind, _key, _payload in read_sections(container.read_bytes())].count(b"XTRA") == 1
