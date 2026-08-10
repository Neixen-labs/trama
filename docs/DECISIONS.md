# Decisions

## 2026-08-09 — Business Source License 1.1 for the core

**Decision:** License TRAMA version `0.0.0-pre-alpha` under BSL 1.1, with Carlos Guerrero as Licensor. Production use is permitted only for internal business operations. Offering TRAMA, or a substantially similar hosted or managed service, to third parties requires a commercial license.

**Change date:** 2030-08-09, when this version converts to Apache-2.0.

**Why:** Keep source visible and permit evaluation, contribution, and internal adoption without allowing an unlicensed hosted competitor during the initial commercial phase.

**Consequence:** TRAMA is source-available, not OSI open source. New released versions must carry their own BSL change date. External contributions require the Individual Contributor License Agreement so the Licensor can relicense future versions.

## 2026-08-09 — Tile-local 16-bit geometry coordinates

**Decision:** Store v0 geometry positions as unsigned 16-bit normalized coordinates relative to each tile extent.

**Why:** This halves geometry coordinate storage versus `f32`, is GPU-friendly, and preserves controllable local precision. At a 10 km tile width, one quantization step is about 15 cm.

**Consequence:** Each tile must declare its extent. Exact engineering values remain typed properties; they are not recovered from quantized render geometry.

## 2026-08-09 — Zstandard compression per section

**Decision:** Compress each v0 section independently with Zstandard (zstd).

**Why:** Independent compression preserves HTTP range loading while providing a strong size/speed trade-off for binary geometry and graph buffers.

**Consequence:** Readers need a zstd decoder; the browser engine will ship or load a small WASM decoder. v0 does not support mixed compression codecs.

## 2026-08-10 — Node identity by snapped coordinate key

**Decision:** Two line endpoints are the same node when their WGS 84 coordinates match after rounding to seven decimal places, roughly one centimetre. The stable node ID is SHA-256 over that rounded key.

**Why:** GeoJSON has no node concept, so identity must come from geometry. Exact float equality splits a network wherever an exporter rounded differently; a rounded key is deterministic, order-independent, and needs no spatial index.

**Consequence:** Endpoints further apart than the grid step stay separate nodes. Sources with sloppier alignment will need an explicit tolerance flag and a spatial index; v0 does not have one.

## 2026-08-10 — Whole-edge tile fitting instead of clipping

**Decision:** Each edge is stored in the deepest tile from z14 down that contains all of its vertices. v0 does not clip geometry at tile borders.

**Why:** It keeps every edge to a single `GEOM` path and one `GeometryRef`, which the specification already permits, and it removes the clipping pipeline from the first useful compiler.

**Consequence:** A long edge drops to a coarse zoom, so a tile set is not a level-of-detail pyramid and a renderer cannot select geometry by zoom alone. Real clipping is required before the engine implements LOD.

## 2026-08-10 — Property types inferred per key, mixtures rejected

**Decision:** Each GeoJSON property key becomes one typed `PROP` column. Booleans map to `bool`, integers to `i64`, numbers with any fractional value to `f64`, and text to `string`. A key seen as both integer and float promotes to `f64`. Any other mixture, a nested value, a non-finite number, or an out-of-range integer is a compile error. JSON `null` means absent, not a value.

**Why:** A typed column is the format's contract, so the type has to be decided at compile time. Silently stringifying a mixed key, or widening everything to `f64`, would hide a modelling error in the source data and lose information the exporters must return.

**Consequence:** Sources with inconsistent property typing must be cleaned before compiling. v0 infers no enum columns: repeated labels are stored as `string` indexes, which already deduplicates them through the string dictionary.

## 2026-08-10 — GeoJSON export writes two documents and preserves edge identity only

**Decision:** `trama export` writes `<name>.nodes.geojson` and `<name>.edges.geojson`. Every feature carries `properties["_trama_id"]`, and compiling an exported edge document takes that ID back rather than deriving a new one. A malformed `_trama_id` fails the compile.

**Why:** GeoJSON holds one FeatureCollection per document, and nodes and edges are separate layers in every downstream tool. Round-tripping without honouring `_trama_id` would renumber entities that never changed, which defeats the point of stable IDs.

**Consequence:** Node identity does not survive a GeoJSON round trip, because a node's position is only recoverable from quantized geometry. Topology does survive: endpoints shared in the source are byte-identical after export, so they re-snap to one node. Exported coordinates sit within one quantization step of the source, so the round trip is not bit-exact geometry.

## 2026-08-10 — EPANET input carries the graph, not the whole model

**Decision:** The `.inp` reader maps `[JUNCTIONS]`, `[RESERVOIRS]`, `[TANKS]`, `[PIPES]`, `[PUMPS]`, `[VALVES]`, `[COORDINATES]`, and `[VERTICES]` into nodes, edges, and named typed properties. Every entity keeps its EPANET ID in a `name` property and its kind in a `type` property. Node coordinates are also stored as exact `x` and `y` properties. `[COORDINATES]` MUST be WGS 84 degrees; anything else is rejected rather than guessed at.

**Why:** The container models a graph with typed properties, and that is all EPANET topology needs. Storing the exact coordinates as properties is what the specification already allows for engineering values, and it makes node positions survive a round trip that quantized render geometry could not.

**Consequence:** `[PATTERNS]`, `[CURVES]`, `[CONTROLS]`, `[OPTIONS]`, `[TIMES]`, `[REPORT]`, and the other simulation sections are not represented, so `trama export --to epanet` cannot bring them back. A `.inp` round trip preserves identity, topology, and per-entity properties, but produces a topology-only model. Link vertices still pass through quantization, so an intermediate vertex returns within about one quantization step. The exported file has not been validated against the EPANET solver itself, only against this compiler.

**Resolved** by the `SRCE` section below, which carries those sections verbatim.

## 2026-08-10 — `SRCE` section for unmodelled source material

**Decision:** Add an optional `SRCE` section, specification `0.2.0`, holding verbatim bytes from the file a container was compiled from, tagged with a format identifier. A reader that does not know the section skips it, so `minimum_reader_version` stays at `0.1.0`. An input adapter SHOULD store only what the container does not otherwise model. The EPANET adapter stores its patterns, curves, controls, options, times, and title, and its exporter replays them around a graph regenerated from `GRPH` and `PROP`.

**Why:** Without it, `.inp` in and out returns a topology-only model, which is not a round trip a water utility can use: their model stops being simulable. Storing only the residue keeps the cost to a few kilobytes instead of duplicating the whole source, which for a large network would outweigh the container itself.

**Consequence:** The graph sections stay authoritative. When stored source material disagrees with them, the exporter MUST prefer the graph, so an edit made through TRAMA is never silently reverted by replayed text. The section is opaque to the core, which keeps the format domain-agnostic: only the EPANET adapter knows what an EPANET pattern is. The stored file name is deliberately empty, so the same model compiles to the same bytes from any path.

**Later:** modelling patterns and curves as first-class named time series in the format, so a solver can consume them without an adapter parsing text back out. That is the durable answer and it belongs after the EPANET solver plugin exists, when its real requirements are known. Until then `SRCE` keeps the data safe but opaque.
