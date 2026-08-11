# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""Reading and writing EPANET `.inp` text.

This module knows the file's shape — bracketed sections of whitespace-separated fields with
`;` comments — and nothing about hydraulics. Section order and raw lines are preserved
because most of a `.inp` travels back out untouched.
"""

from __future__ import annotations

from typing import NamedTuple


class Document(NamedTuple):
    """Sections in file order. A name may repeat: Net3 declares `[REACTIONS]` twice."""

    sections: list[tuple[str, list[str]]]

    def lines(self, name: str) -> list[str]:
        return [line for section, body in self.sections for line in body if section == name]

    def rows(self, name: str) -> list[list[str]]:
        """Field rows of a section, with comments and blank lines dropped."""
        return [fields for line in self.lines(name) if (fields := values(line))]

    def without(self, names: set[str]) -> Document:
        return Document([(name, body) for name, body in self.sections if name not in names])


def parse(text: str) -> Document:
    sections: list[tuple[str, list[str]]] = []
    body: list[str] = []
    name = ""
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            sections.append((name, body))
            name, body = stripped[1:-1].strip().upper(), []
        else:
            body.append(line)
    sections.append((name, body))
    # The leading group holds whatever preceded the first header, usually nothing.
    return Document([entry for entry in sections if entry[0] or any(line.strip() for line in entry[1])])


def serialize(document: Document) -> str:
    parts = []
    for name, body in document.sections:
        parts.append(f"[{name}]\n" if name else "")
        parts.extend(f"{line}\n" for line in body)
    return "".join(parts)


def values(line: str) -> list[str]:
    """The fields of one line: everything before `;`, split on whitespace."""
    return line.split(";", 1)[0].split()


def section(name: str, header: str, rows: list[list[str]]) -> tuple[str, list[str]]:
    """A section built from field rows, laid out the way EPANET's own writer does."""
    return name, [f";{header}", *(" " + "\t".join(fields) for fields in rows), ""]


def number(value: str) -> float:
    return float(value)


def text(value: float | str) -> str:
    """Render a value back into a field, without the trailing `.0` EPANET never writes."""
    if isinstance(value, str):
        return value
    return str(int(value)) if value == int(value) else repr(value)
