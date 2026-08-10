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
