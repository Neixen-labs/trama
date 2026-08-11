# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The `.inp` reader keeps everything it does not understand."""

from pathlib import Path

from trama_epanet import inp

NETWORKS = Path(__file__).parent / "networks"


def test_a_file_survives_parse_and_serialize_unchanged() -> None:
    text = (NETWORKS / "Net3.inp").read_text()

    assert inp.serialize(inp.parse(text)) == text


def test_a_repeated_section_is_not_collapsed() -> None:
    document = inp.parse("[REACTIONS]\n Order Bulk 1\n\n[REACTIONS]\n Global Bulk -0.5\n")

    assert [name for name, _body in document.sections] == ["REACTIONS", "REACTIONS"]
    assert document.rows("REACTIONS") == [["Order", "Bulk", "1"], ["Global", "Bulk", "-0.5"]]


def test_comments_and_blank_lines_are_kept_in_the_text_but_not_in_the_rows() -> None:
    document = inp.parse("[JUNCTIONS]\n;ID\tElev\n 10\t710\t;a trailing note\n\n")

    assert document.rows("JUNCTIONS") == [["10", "710"]]
    assert ";ID\tElev" in inp.serialize(document)


def test_a_number_is_written_the_way_epanet_writes_it() -> None:
    assert inp.text(10530.0) == "10530"
    assert inp.text(0.5) == "0.5"
    assert inp.text("Open") == "Open"
