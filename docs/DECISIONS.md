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

## 2026-08-10 — Line geometry ships centerlines, not a mesh

**Decision:** A writer does not tessellate line geometry. For a tile whose paths are all lines, `mesh_vertex_count` and `mesh_index_count` are zero and the renderer builds the ribbon from consecutive `PathVertex` pairs on the GPU. The mesh fields stay for geometry that needs real triangulation.

**Why:** `MeshVertex` records a position and an edge, not a side of the centerline, so it cannot express a ribbon of constant screen-space width. The alternatives were to widen the vertex with a side flag and a quantized normal, or to bake thickness in world units. The first costs roughly four times the vertices of the path array on line-heavy data; the second makes a line vanish when zooming out and forces a recompile to restyle.

**Consequence:** Pillar 1's "pre-tessellated, ready for `bufferData`" holds for polygons but not for lines, where the file ships centerlines and the GPU reconstructs the ribbon. Stroke width becomes a camera-time and style-time decision. The engine owes a line renderer that instances quads from `PathVertex`; a reader must accept a zero mesh as valid.

## 2026-08-10 — The reference solver speaks HTTP and Server-Sent Events, not WASM

**Decision:** Implement the first solver, `solvers/example-diffusion`, as an HTTP server streaming packed deltas over Server-Sent Events. The WASM/WASI runtime the contract also allows comes later.

**Why:** Both transports must produce the same 18-byte delta, so the contract can only be proven by an implementation that exercises the parts a mock cannot: a stream that arrives in arbitrary chunks, an error raised mid-solve, and a client that must reject a stream ending without its completion event. A WASM module returning a buffer in one piece exercises none of those. The server also runs unchanged in the tests, so the suite talks to real sockets rather than to a fake of the protocol.

**Consequence:** The engine's solver client is written against a stream, which is the harder of the two shapes; a WASM runtime can hand it the same bytes without changing it. Nothing yet proves the sandboxed path, and no solver is isolated from the host — an HTTP solver is trusted code at the other end of a socket. The event framing is now load-bearing and belongs in `docs/SOLVER_CONTRACT.md`, not in the example's source.

## 2026-08-11 — The producing side is written in Rust

**Decision:** The compiler, the CLI, the solvers and the browser module are Rust. `compiler/` and `solvers/**` are removed. The rendering engine stays TypeScript, so the repository holds two languages rather than three. This replaces the stack `CLAUDE.md` fixed as an owner constraint, and the owner made the change knowingly.

**Why:** Distribution was the weakest part of the project. Running the EPANET solver needed Python plus a C toolchain — `owa-epanet` publishes no Windows wheel and none for macOS on 3.12 — and the phase 5 playground needs the compiler in a browser, where Python costs 5.8 MB of Pyodide against 121 kB of Rust compiled to wasm. The alternative to a rewrite was a second compiler in TypeScript, which is two implementations of a deterministic format that must agree byte for byte. Doing it at 600 lines was judged cheaper than doing it later at several thousand.

**Consequence:** Every future format change is written in a language the owner does not use daily, which is a bet about maintenance more than about technology. Publishing changes shape: a pure-Python package installed everywhere instantly, and native binaries must be built per platform instead — the problem `owa-epanet` had, now ours. The port also had to reproduce Python's rounding of halves to even and zstd's content-size field, both of which turned out to be spec gaps rather than porting details (#89, #90). Equivalence was checkable rather than argued: the container, the GeoJSON export, the `.inp` export and the demo fixture are all byte-identical to what Python produced, and the EPANET round trip still passes on Net1 and Net3.

## 2026-08-11 — The frame benchmark reports rather than gates

**Decision:** `engine/bench` measures 100k segments with animated state and prints frame cost, cadence and late frames. Its workflow runs on `main` and on demand, uploads the rendered frame, and fails only when the harness breaks — never on a frame time. `FRAME_BUDGET_MS` turns the budget into an exit code for whoever runs it on real hardware.

**Why:** `KICKOFF.md` asks for 60fps on a mid-range phone. A hosted runner has no GPU, so its numbers are SwiftShader's; failing a pull request on them would report the runner rather than the change, and passing would be a green check on a claim nobody verified. Two guards do the gating instead, both of which catch a benchmark flattering itself: the run fails if no frame ever bound the state texture, and `--screenshot` makes "it drew nothing, very fast" disprovable.

**Consequence:** No automated protection against a rendering regression; a human runs the benchmark and reads the numbers. Acceptable while the measured cost is 0.6 ms against a 16.7 ms budget, and revisited if that margin ever narrows. The phone target stays unverified and is an owner task, since it needs a real device.

## 2026-08-11 — Graph identities are stored as ascending deltas

**Decision:** `Node` and `Edge` records no longer carry a `u64 id`. Each array's identities become a block of unsigned LEB128 varints holding the first id and then the gap to each predecessor, addressed from two new header offsets (SPEC 4.1). The specification moves to 0.3.0.

**Why:** The phase 3 size criterion is under 20% of the equivalent GeoJSON and the compiler sat at 20.4%. `GRPH` is 71% of a file, and its identities are the part no compressor can help with: an id is the first 8 bytes of a SHA-256, so on a 49,612-edge network 597 kB of ids compress to exactly 597 kB. The format already requires both arrays sorted by ascending id, and sorted values have gaps around `2^64 / n` — six bytes rather than eight. Measured: 597 kB stores as 509 kB, 88 kB off the file, 4.5% of its total size.

**Consequence:** An id can no longer be read at a fixed offset; recovering the `i`th requires decoding the block up to it. Nothing needs that — a reader that wants ids wants all of them, to build the map from id to index — but a future reader that streams one entity would have to change. Record layouts changed incompatibly, so this is not a file a 0.2.0 reader can open, and every existing container must be recompiled. The saving is bounded and known: it is the only incompressible mass in the format, and there is no second one to find later.

## 2026-08-11 — Domain leftovers ride in an opaque section the core cannot read

**Decision:** Add the optional `XTRA` section kind (SPEC 7): bytes with a declared owner and media type, which the core stores, compresses, checksums, and range-serves but has no code to parse. EPANET's patterns, curves, controls, rules, options, and unit system travel there. The `.inp` parser lives in `solvers/epanet/` behind an importer interface the compiler discovers, and an importer must be told the coordinate reference system rather than infer one.

**Why:** `KICKOFF.md` requires a functional `.inp` round trip; `CLAUDE.md` forbids a domain concept outside `solvers/`. Roughly a third of an `.inp` has no entity to attach to, and `[CONTROLS]` and `[RULES]` are a small imperative language rather than data, so no amount of typed-property design absorbs them. Extending `PROP` with list types would grow the core format for one domain and still fail to reach the language. Keeping the `.inp` alongside as the source of truth would abandon the single-portable-file pillar for the first domain the project ships. An opaque section makes the core agnostic by construction rather than by discipline: there is no code that could interpret those bytes, so no plausible-looking future commit can start interpreting them.

**Consequence:** A file may now contain a region only one solver understands, and that region is a tempting place to put anything inconvenient — the guard is that `XTRA` MUST have the required bit clear, and a file MUST decode, render, and traverse identically with every `XTRA` record removed. References point one way: a payload may name entities by stable ID, and nothing else in the file may refer to `XTRA`. Adding an optional section kind is a minor addition under SPEC 10, so the specification is now 0.2.0 and a reader written against 0.1.0 ignores the record instead of rejecting the file. The compiler gains a plugin seam it did not have, which GeoPackage and CSV importers will reuse.

## 2026-08-11 — Node identity comes from the geometry grid

**Decision:** When a source does not name its nodes, a writer derives node identity from the section 3.1 quantization cell, expressed globally as `tile * extent + q`, instead of comparing coordinates for equality. Recorded in SPEC 4.2.

**Why:** Identity by exact float equality means two features share a node only when their coordinates are bit-identical. Shared vertices written by different tools, or round-tripped through different precisions, routinely differ in their last digits, and the result is a graph torn into fragments whose geometry still draws perfectly — the failure is invisible to rendering and only surfaces when a solver traverses topology. The alternative, snapping within a tolerance, is what GIS tools do, but it puts a dataset-dependent number in the middle of topology: too large and it silently merges genuinely distinct nodes. The grid needs no tuning because it is the precision the file already stores.

**Consequence:** Topology is fixed at about 4 cm at the equator at `z14`, and two nodes closer than one cell become one node; a source needing finer topology must name its nodes. Node IDs derived from position change with this decision, so existing containers do not compare equal to a recompilation of their source. Quantization is now load-bearing for identity as well as for rendering, so a future change to `extent` or to the tile zoom would renumber every unnamed node. A `--node-tolerance` escape hatch stays available if a real dataset needs one.
