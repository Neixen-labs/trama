# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The manifest must promise exactly what the importer can declare, per contract 2.1."""

import tomllib
from pathlib import Path

import pytest

from trama_epanet import inp
from trama_epanet.importer import FLOW_UNITS, PRESSURE_UNITS, channels

MANIFEST = tomllib.loads((Path(__file__).resolve().parents[1] / "solver.toml").read_text())
OUTPUTS = {output["channel"]: output for output in MANIFEST["outputs"]}


def _declared(flow_units: str) -> dict[str, str]:
    document = inp.parse(f"[OPTIONS]\n Units {flow_units}\n")
    return {channel["name"]: channel["unit"] for channel in channels(document)}


@pytest.mark.parametrize("flow_units", FLOW_UNITS)
def test_every_unit_the_importer_can_declare_is_one_the_manifest_promises(flow_units: str) -> None:
    """The two would drift apart silently: the mismatch only shows up at solve time."""
    declared = _declared(flow_units)

    assert declared["flow"] in OUTPUTS["flow"]["units"]
    assert declared["pressure"] in OUTPUTS["pressure"]["units"]


def test_the_manifest_promises_nothing_the_importer_cannot_produce() -> None:
    assert set(OUTPUTS["flow"]["units"]) == set(FLOW_UNITS)
    assert set(OUTPUTS["pressure"]["units"]) == set(PRESSURE_UNITS)


def test_us_units_report_pressure_in_psi_and_si_units_in_metres() -> None:
    assert _declared("GPM")["pressure"] == "psi"
    assert _declared("LPS")["pressure"] == "m"


def test_an_unknown_flow_unit_is_refused_where_the_file_is_still_in_hand() -> None:
    with pytest.raises(ValueError, match="not an EPANET flow unit"):
        _declared("furlongs")


def test_the_manifest_declares_the_contract_version_whose_field_it_uses() -> None:
    # `units` arrived in 0.2.0; claiming only 0.1.0 while using it would be a manifest a
    # conforming 0.1.0 host must reject.
    assert "0.2.0" in MANIFEST["contract_versions"]
    assert all("unit" not in output for output in MANIFEST["outputs"])
