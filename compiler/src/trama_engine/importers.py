# SPDX-License-Identifier: LicenseRef-BSL-1.1
"""The seam through which a source format the core does not know reaches the compiler.

A format that carries domain meaning — EPANET's `.inp` first — is read by the package that
owns that domain, never by this one. An importer hands back GeoJSON features, which the
compiler already speaks, plus whatever the format would otherwise lose as opaque records.
"""

from __future__ import annotations

from collections.abc import Mapping
from importlib.metadata import entry_points
from pathlib import Path
from typing import Any, NamedTuple, Protocol, runtime_checkable

from trama_engine.compiler import Extra

GROUP = "trama.importers"
NATIVE_SUFFIXES = (".geojson", ".json")


class Import(NamedTuple):
    """What an importer produces: things the format can express, and things it cannot."""

    features: list[dict[str, Any]]
    # No default: an importer that loses nothing should have to say so, since the alternative
    # is dropping a pattern or a curve without ever noticing.
    extras: list[Extra]
    # What a container built from this format can be solved for. STCH is a declaration, never
    # data, and which channels a format implies is the importer's knowledge, not the caller's.
    channels: list[dict[str, Any]]


@runtime_checkable
class Importer(Protocol):
    """Reads one family of source files. Implementations live outside this package."""

    suffixes: tuple[str, ...]

    def load(self, source: Path, options: Mapping[str, str]) -> Import: ...


def find(source: Path) -> Importer | None:
    """The installed importer claiming this suffix, or `None` when nothing does."""
    suffix = source.suffix.lower()
    return next((importer for importer in _installed() if suffix in importer.suffixes), None)


def _installed() -> list[Importer]:
    return [entry.load()() for entry in entry_points(group=GROUP)]


def parse_options(pairs: list[str]) -> dict[str, str]:
    """Turn `-o key=value` into a mapping, rejecting the malformed before any work starts."""
    options = {}
    for pair in pairs:
        key, separator, value = pair.partition("=")
        if not separator or not key:
            raise ValueError(f"option {pair!r} is not key=value")
        options[key] = value
    return options
