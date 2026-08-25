# trama-roads

Reads a road network out of an OpenStreetMap extract, for TRAMA. It translates every spelling of
`oneway`, normalises `maxspeed` into a speed column, splits ways at the junctions they cross, and
declares the channel a router writes.

It also reads turn restrictions: an OSM `type=restriction` relation becomes a column naming, for
each street, the *runs* of streets it may not be followed by — `only_*` expanded to the
prohibitions it implies, so a router asks one question and never learns which spelling produced
the answer. A `via` that is a way rather than a node, the no-U-turn across a dual carriageway, is
walked piece by piece and written as the run it forbids.

Part of [TRAMA](https://trama.build). Licensed under the
[Business Source License 1.1](https://github.com/Neixen-labs/trama/blob/main/LICENSE).
