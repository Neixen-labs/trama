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

## 2026-08-11 — The specification names what two conforming writers were free to disagree on

**Decision:** Two clarifications, no binary-layout change, so the specification moves to 0.3.1. A section's zstd frame MUST declare its decompressed size in its own header, and that size MUST equal the directory's `uncompressed_bytes` (SPEC 8). Quantization rounds half to even (SPEC 3.1).

**Why:** Both were found by porting the compiler from Python to Rust, and both had the same shape: the text was silent, the two implementations chose differently, and neither was wrong by the letter. Rust's streaming `encode_all` omits the frame content size where Python's one-shot `compress` embeds it, and `zstandard`'s `.decompress()` refuses a frame without it — so the first Rust container was rejected by our own reference reader while both sides emitted valid zstd. Rounding was quieter and worse: Python rounds half to even, while `f64::round`, C's `round`, and `Math.round` all move away from the tie, and SPEC 4.2 derives node identity from the quantized cell. Two writers disagreeing there give the same input different stable IDs. Half-to-even is what IEEE 754 defaults to and what the reference implementation already does, so stating it keeps every existing file valid.

**Consequence:** Writers on a streaming compression API must pledge the size or use a one-shot call — one line in the Rust port, which `zstd::bulk::compress` already satisfied. Writers on a language whose `round` moves away from ties must say half-to-even explicitly, as `core/trama-format/src/write.rs` does. No file changes and no reader needs updating; a reader MAY now reject a frame that omits its size before allocating for it. The general lesson is worth more than either fix: a second independent implementation is what turns "the spec does not say" from a theoretical gap into a rejected file, and the remaining silences in the text are the ones nobody has ported against yet.

## 2026-08-11 — The fly-through aims ahead of itself instead of fitting a spline

**Decision:** The camera follows the graph as a polyline in Web Mercator metres and takes its heading from a point further along the route rather than from the segment it is on. `KICKOFF.md` asks for a camera on a spline of the graph; there is no spline. Three supporting choices: the tour backtracks, the zoom comes from the route's length, and distances stay in Mercator metres.

**Why:** A polyline's heading jumps at every vertex, which is the actual complaint a spline answers. Aiming at a point a fixed time ahead answers it too — the heading starts turning before the corner, the way a driver does — and it is one line of arithmetic against a curve fit plus its tangent. The look-ahead is also the only tuning knob, and it means something physical: how far ahead the camera looks. A spline would have added a knot-placement question nobody asked.

Backtracking was not a refinement but a correction. A tour that merely refuses to repeat an edge stops at the first node whose edges are all visited: measured on Net3, five edges out of a hundred and nineteen, because 42 of its 97 nodes have degree three. Retracing the edge it arrived by turns that into all 119, crossed at most twice. A camera cannot teleport, so covering the network and staying continuous are the same requirement.

The zoom is derived because a fixed one is wrong by orders of magnitude on real data. An EPANET `.inp` declares no coordinate system and often carries arbitrary units, so Net3 compiles to a network 425 m across; a municipal network is tens of kilometres. One zoom cannot frame both, and the flight is a fixed sixty seconds regardless.

**Consequence:** Distances are Mercator metres, not ground metres — about 30% long at latitude 40 — which scales apparent speed and nothing else, so nothing corrects for it. The tour is depth-first, so its order is whatever the CSR lists first and its length up to twice the network's; a shorter one is a route-inspection problem and no criterion asks for the optimum. `walk` now returns edges more than once, which any future consumer counting edges must expect. This closes the phase 4 "criterio de hecho": the demo loads a `.trama`, scrubs a day of solver state over it, and flies the network.

## 2026-08-11 — A directed edge is declared by a reserved key, not inferred from the domain

**Decision:** A GeoJSON feature declares its edge directed with `properties["_trama_directed"] = true`, joining `_trama_id` as the second and last reserved key (SPEC 9). Direction is the stored vertex order; a source that declares the reverse must reverse the LineString. Neither reserved key becomes a `PROP` column. The specification moves to 0.4.0.

**Why:** `Edge.flags` bit 0 has meant `directed` since 0.1.0 and nothing has ever set it — the compiler writes `flags = 0` and pushes both adjacency entries for every edge. The missing half was never the bit but the input: no way for a source to say so. This surfaced on the way to a second domain, where it stops being cosmetic. A routing solver that ignores one-way streets does not return a slightly worse route; it returns one that drives against traffic, and the map still draws correctly, so the error is invisible exactly where section 4.2 warns that spatial errors hide.

The alternative was a `-o directed-from=<property>` option pointing the compiler at an existing field. It reads an OSM extract with no preprocessing, which is real convenience, and it was rejected anyway: deciding that `yes`, `1` and `true` mean directed while `-1` means directed-but-reversed is road knowledge, and it would live in the core the moment the core interprets it. It also loses on export, which cannot know which property produced the flag. Leaving it to a domain importer, as EPANET's `.inp` reader is left, does not replace this: that importer builds GeoJSON internally and needs the same key to write through.

**Consequence:** Producers must translate their own concept — `oneway`, a pump's flow direction, a valve's — into the key, and a road extract needs a line of preprocessing. Reversal is expressed by reversing geometry, so the same physical street yields different stored vertex order depending on its direction, and its rendered line is unchanged. Adding a reserved key is a backward-compatible addition under SPEC 10, and since no writer emits the flag today, every existing container is unaffected and a 0.3.x reader already knows what the bit means. The key is a namespace claim on producer data: `_trama_directed` is now forbidden as an ordinary property name.

## 2026-08-11 — Routing is the second domain, and the CSR already enforces its one rule

**Decision:** A new `core/trama-routing` crate, sibling to `trama-epanet`, implementing the solver contract: Dijkstra over the CSR adjacency, cost from geometric length, waypoints as node indices, and output as an edge channel whose value is 1 once the vehicle has passed. Direction gets no special case in the search.

**Why:** `KICKOFF.md` claims a domain-agnostic core, and until now one domain stood behind that claim. Water alone cannot show whether `GRPH`, `PROP` and state channels are general or merely hydraulic in disguise. Routing is the cheapest second domain that is genuinely different: it reads topology rather than writing per-entity scalars, and its result is a path rather than a field.

The delta format needed nothing added. A route is `(entity_id, channel, t, value)` already: the edges it crosses, and the instant the vehicle reaches each. That means the temporal scrub built for pressure animates a vehicle with no new rendering code, which is a stronger statement about the design than any argument for it.

Direction turned out to need no code at all. SPEC 4 gives a directed edge one CSR entry, at its source, so an edge that may not be crossed backwards does not appear among what leaves its target. The search cannot violate the restriction because the topology never offers it. A router that consulted `Edge.flags` would be re-deriving what the adjacency array already states.

Cost is geometric length because the geometry is always present. A property column would have been more useful for roads — travel time wants a speed — and more fragile, since a container without that column has no cost at all. Length is the honest default and the parameter for a named cost column can arrive when a container carries one.

**Consequence:** The route is the shortest, not the fastest, and no parameter yet changes that. Waypoints are node indices into an array sorted by stable ID, so they do not match input order and are opaque to a human reading them; a client reads them from the graph, which is what a map click does anyway. Solving requires a container declaring the channel, and only a domain importer declares channels — a GeoJSON compiles with none — so today that means `--channels` on the command line. The playground therefore cannot route yet: it needs either a road importer that declares `on_route` or a way for the page to declare one, and it needs a road network to run on. Length inherits the file's precision, about 4 cm at `z14`, which is far below any routing decision. VRP stays out: it is NP-hard and would bring a heuristic or a heavy dependency, where shortest-path demonstrates the same thing at two orders of magnitude less code.

## 2026-08-11 — A road importer, reached by name, that splits ways at their junctions

**Decision:** A `core/trama-roads` crate reading an Overpass `out geom;` extract. It translates all four spellings of `oneway` into `_trama_directed`, stores a street tagged `-1` with its geometry reversed, namespaces every tag as `osm:*`, declares the `on_route` channel, and splits each way where another way touches it. The `Importer` trait gains `id()`, and `trama compile --importer <id>` overrides suffix detection.

**Why the flag:** selection was by suffix alone, and OpenStreetMap's export is `.json` — already claimed by the compiler as "compile this as it stands". Without a way to ask for the importer, a road network would compile as plain GeoJSON and every one-way street would silently become two-way. The alternative was a compound suffix like `.osm.json`, rejected because a file that behaves differently depending on what it is named is a surprise waiting for whoever renames it; and teaching the native path to translate roads, rejected because `CLAUDE.md` keeps domain knowledge out of the core. With the flag absent nothing changes for a file that compiled before.

**Why the splitting:** this was found by running the thing rather than by reasoning about it. The first real extract produced a graph where a sampled node could reach 12 others out of 2,500 pairs tried, and the longest route was two edges. SPEC 4.2 takes a node from each end of a LineString and nowhere else, while OSM records a crossing as a shared node that frequently sits mid-way through at least one of the ways. Handed over whole, streets cross on the map and never meet in the graph — the same class of failure section 4.2 already warns about, arriving through a different door. Counting node references across the extract finds the junctions: on a 556-way extract of central Madrid, 250 shared nodes sat mid-span, and splitting there took the network from unusable to 806 edges where a typical node reaches 531 of 617.

**Consequence:** The importer requires `out geom;`, since a query without it returns node references with no positions, and says so when a way arrives without geometry. A way whose node list does not match its geometry — which post-processing produces — is left whole rather than dropped: a worse graph, still a true one. Edge identity is `osm:way/<id>/<piece>`, stable while the way's node list is, so re-splitting a re-tagged way renumbers its pieces. `reversible` is treated as two-way because the format has nowhere to put a schedule. Turn restrictions, which OSM stores as relations, are ignored entirely: the graph cannot express them and a router that honoured them would need somewhere to read them from. Measured on real data: 806 edges, 686 of them one-way, and of 29 routes sampled, all 29 had a different return trip.

## 2026-08-11 — The playground routes, and the clock comes from the answer

**Decision:** `trama-wasm` gains `compile_osm` and `solve_route`, the playground offers "ruta más corta" with waypoints picked by clicking the map, and `fixtures/madrid.osm.json` ships as a preloaded example under ODbL attribution. After solving, the scrub's range is cut to the moment the route completes rather than the window the solver was given.

**Why:** the second domain existed only on the command line, where nobody meets it. This closes the phase 5 criterion for a domain that is not water: a stranger opens the page, loads a street network, taps two points, and watches a vehicle take a route that respects one-way streets — with nothing installed and no data leaving the browser.

The routing solver needed no WASI, unlike EPANET. It is Rust with no filesystem in its API, so the same code the server runs compiles straight into the existing module. That the hydraulic solver needed a virtual filesystem and this one needed nothing is a fact about EPANET's C API, not about the contract.

Cutting the scrub was found by looking at the result rather than the counter. At 8 m/s a half-hour window covers 14 km and these streets are two, so the whole journey happened in the first inch of the slider: the deltas were right, the counter said 1,953, and dragging the control did nothing visible. The stream is the only thing that knows when the journey ends, since the solver reports against a clock the caller chose, so the page reads the arrival time out of the deltas it just applied.

**Consequence:** The example carries OpenStreetMap data, so ODbL attribution now appears in the playground and in `fixtures/README.md`; that licence governs the extract and anything derived from it, not TRAMA's own source. The module grew to 319 kB brotli from 121 kB, which buys three importers and two solvers in the browser — the README carries the measured figure rather than the old one. Speed is a constant 8 m/s because cost is still geometric length and nothing reads `osm:maxspeed`, which rides along as a property waiting for a reader. Waypoints are picked by nearest node to a click, so a tap far from any street still lands somewhere; a tolerance would need a real complaint to tune against.

## 2026-08-11 — Travel time comes from a column the importer writes, not from a tag the solver reads

**Decision:** `trama-roads` derives a `roads:speed_ms` property for every edge, from `maxspeed` where it exists and from the road class where it does not. `trama-routing` takes a `speed_property` parameter naming a column of metres per second; with one it costs a traversal in seconds and minimises time, without one it minimises distance. `trama-format` exposes `edge_properties`. Route costs and `reached_at` are now seconds throughout.

**Why:** "shortest" is not what anyone means by a route. Making it "fastest" is one division, but where the division happens decides whether the core stays clean. The solver must not know that `maxspeed` is a key, that OSM counts in km/h, that `30 mph` is a different unit or that `walk` is a speed at all — that is road knowledge, and it lives in the road importer. What crosses the boundary is a number in a stated unit and a parameter naming the column that holds it. A solver reading `osm:maxspeed` directly would have been fewer lines and would have put OpenStreetMap's vocabulary inside a domain-agnostic search.

The fallback by road class is a heuristic and could not be avoided: only 43% of the ways in the sample extract carry `maxspeed`, and every one that does not is a `residential`, `living_street` or `tertiary` street. Without it more than half a city has no cost. An unparseable value falls back rather than reading as zero, because a zero speed is an infinitely slow edge and `signals` and `variable` appear in real data.

**Consequence:** The fallbacks are urban Spain, where `residential` has been 30 km/h since 2021. They are wrong for a motorway network and wrong for most other countries; a real answer would come from the extract's country, which an extract does not state, so the constant carries a `ponytail:` naming its own limits. An edge whose column holds nothing usable falls back to the caller's speed rather than failing, so a partly-tagged network routes rather than refusing. `Route.reached_at` changed meaning from metres to the unit of the costs handed in, which is a silent change for anyone holding that struct. Measured on the sample extract: 9 of 52 node pairs take different streets when measured in time, and the playground's window fell from 480 s to 420 s for the same journey once real speed limits replaced a flat 8 m/s.

## 2026-08-12 — Range reads are cached in OPFS, keyed by the caller and never revalidated

**Decision:** `cachedInOpfs` wraps a `RangeReader` and stores each range under a caller-supplied key in the origin private file system. It degrades to the reader it wraps wherever OPFS is missing or denied, and a failed write never fails a read that already succeeded. `forget(key)` drops one container's ranges.

**Why:** the first pillar promises a file that works offline, and until now every load went to the network. Caching ranges rather than whole files is what the format was shaped for: a reader asks for the header, the directory and the tiles in view, so what lands on disk is what was looked at. Measured on the demo, a second load of the same container transferred 0 bytes and made 0 range requests, against 561 bytes over 3 ranges cold.

Nothing is revalidated. Checking freshness needs a request, which is the one thing unavailable offline, and SPEC 10 already says a new dataset version is a new file with a new `file_uuid` — an immutable container is the format's own model. The cost is that a file republished at the same URL serves the old bytes until `forget`, and that is stated rather than papered over.

The key is the caller's because the reader does not know what it is reading: it sees byte ranges, and the identity of the container is a fact the page has and the range reader does not. A key derived from the URL would have been convenient and wrong for a reader that is not HTTP-backed.

**Consequence:** Every failure mode degrades to the uncached behaviour rather than to an error — no OPFS, denied storage, a full disk. That is deliberate: a container that only loads when a cache is available would be a worse product than one that loads slowly. Nothing evicts, so a browsing session across many containers grows until the origin's quota complains and writes start failing, at which point reads still work. The cache is per-origin and per-key, so two containers never mix. This is the storage half of offline; the page and its modules still come from the network, which a service worker is for.

## 2026-08-12 — The playground precaches itself, and its asset list is generated

**Decision:** A service worker precaches the playground on install and serves it cache-first. `build.sh` writes `sw.js` from `sw.template.js`, filling in the list of files it just produced and a version derived from their bytes. Requests to other origins are never intercepted.

**Why:** OPFS caches what a container is made of; this caches what reads it. Neither alone delivers the first pillar's offline promise, and the pair does: with the server shut down — not emulated offline, shut down — the page loads, compiles a 206 kB OpenStreetMap extract and routes across it.

The list is generated because it cannot be written by hand and stay true: whether the EPANET module is in the build at all depends on whether the build had a WASI SDK, and `build.sh` already skips it with a warning rather than failing. A hand-kept list would have been correct until the first build without one. The version is the bytes' own digest, so a deploy that changes nothing keeps its cache and one that changes anything replaces it, with the old cache deleted on activate.

Cross-origin requests pass straight through. A solver on another host is a live thing to talk to, not an asset; serving it from a cache would answer today's question with yesterday's result.

**Consequence:** Install fetches about 5.4 MB, all of it, before the worker activates — a partial precache is an offline page that fails halfway through a task, which is worse than one that never claimed to work. Registration happens after the compiler is ready, so precaching does not compete with the first load, and it is skipped entirely where `navigator.serviceWorker` is absent. `demo/sw.js` is a build artefact and gitignored beside `vendor/`, so a checkout without a build has no worker at all rather than a stale one.

Three of my own test harnesses were wrong before this was proven: Playwright's `setOffline` rejects a navigation before the worker sees it, so it cannot test one; `waitForFunction` with an async page function passes immediately, since a pending promise is truthy, which reported an empty cache as full; and a probe server returning `application/octet-stream` made the browser download the page instead of opening it.

## 2026-08-12 — What the playground compiles, it hands back

**Decision:** Once a network compiles, the playground offers the `.trama` itself and its GeoJSON export as downloads. The export is two files, `nodes` and `edges`, and `trama-wasm` grew `export_nodes` and `export_edges` over the `trama_format::export` the command line already used.

**Why:** the first pillar says the container is one portable file, and until now the page was the only place it existed. Compiling a network and being unable to keep it makes the playground a demonstration rather than a tool — and the file is the product, not the picture of it.

Two files because SPEC 9 says an export is two FeatureCollections. One mixed collection would have been friendlier to a browser that saves one file at a time, and would have been the page inventing format, which rule 2 forbids. Two functions rather than one returning both for the same reason: the shape follows the spec, not the download dialog.

**Consequence:** The round trip closes inside the browser. Verified on the Madrid extract: 806 edges out, 686 of them carrying `_trama_directed`, and dropping `madrid.edges.geojson` straight back on the page recompiles to the same 617 nodes and 806 edges — the nodes are not in that file at all, they come back from the section 4.2 quantization cell, which is the identity rule doing exactly what it was written for. The container on disk is byte-identical to the one the page reported. Nothing is uploaded to produce any of it.

## 2026-08-12 — The panel folds, and `hidden` was never hiding anything

**Decision:** `.row:not([hidden])` replaces `.row` in the playground's stylesheet, the panel is bounded by the viewport with its own scroll, and it folds to its header, its stats and the rows that act on the map. On a screen under 640px it folds itself as soon as there is something behind it worth seeing.

**Why:** on a phone the panel was 144% of the screen at rest and 161% while simulating, with the map entirely behind it. The cause was not the layout: `.row { display: flex }` is an author rule and `[hidden]` comes from the browser's own stylesheet, so the author rule won and every row the page believed it was hiding had been visible all along, on every screen. The panel was carrying five rows it thought were gone. Fixing the attribute is the root-cause fix; it shortens the panel everywhere, not only on phones.

What survives the fold is what still has a job while you are looking at the map: flying, picking waypoints, scrubbing time, and the stats that say what the network turned out to be. What disappears is input — dropping a file, choosing an example — which by definition you have finished doing.

**Consequence:** Measured at 390x664: the panel goes from 161% of the screen to 49% while simulating, leaving 325px of map, and the scrub still works folded. Folding is remembered by nothing; a new file expands the panel again through the same automatic path. `build.sh` now includes `index.html` in the cache version, without which a page-only change keeps its cache and never reaches anyone with the service worker already installed — verified by changing the file and watching the digest move.

## 2026-08-12 — The phone gets the playground back, behind one tap

**Decision:** The landing shows the embedded playground below 640px again, in a frame of `min(64dvh, 520px)` with an overlay that holds the touch gesture until it is tapped. The full-screen link stays. In a window under 560px tall the folded panel also drops its stats.

**Why:** the frame was hidden on phones because the playground's panel covered the map, which the fold has now fixed. What has not changed is that a map filling a phone screen swallows the swipe meant to scroll the page, and a visitor trapped in an embed is worse off than one who never saw it. The overlay is the ordinary answer, and it costs one tap, removed for good once given — taking the gesture back later would be the greater surprise.

Dropping the stats in a short window is the same reasoning as the fold itself: inside a 425px frame the folded panel was still 323px, leaving 102px of map. What the network turned out to be can wait; seeing it cannot.

**Consequence:** Measured on an iPhone 13 viewport: 0 requests into `/demo/` before the section is reached, the page still scrolls 500px with the pointer over the frame, and after the tap the frame compiles Net3 with the panel at 187px of 425. Desktop is untouched — no overlay, no fold, stats visible. The overlay's rule is written `.playground-tap:not([hidden])` for the reason the last commit found the hard way: a plain `display` rule beats the `hidden` attribute.

## 2026-08-12 — The playground opens a container, and offers only one download

**Decision:** The playground accepts a `.trama`, reads dropped files as bytes, and passes a container through without compiling it. The two GeoJSON download buttons are gone, along with `export_nodes` and `export_edges` in `trama-wasm`; one button downloads the container and a line of prose points at `trama export` for GeoJSON.

**Why:** the download button added the day before handed out a file the page itself rejected with `expected value at line 1 column 1`. With no published CLI binaries, `@trama/core` still `private: true`, and no hosted solver, a `.trama` was usable only by whoever produced it on the machine that produced it — which is the first pillar failing in the one place a visitor can check it.

Three equal buttons also said the wrong thing. They read as "pick a format" when the claim is that the container *is* the format, and the GeoJSON was 599 kB against the container's 43 kB — offering the same map fourteen times larger. Export is still a real promise, and a promise about not being trapped is better kept by the command line than by a button competing with the thing being demonstrated. Removing the two WASM exports saved 19,411 bytes, under 1%: the weight was never the argument.

**Consequence:** Verified end to end — Madrid compiles to 43 kB, downloads as 44,069 bytes, reopens with no compile step at 617 nodes and 806 edges, and *routes on the reopened file*, 1,705 deltas from a container the page never compiled. A file renamed to `.trama` is refused by its magic with a sentence rather than a stack trace. This is the minimum for the format to exist for anyone but its author; published binaries and a published package are still missing.

## 2026-08-12 — The camera follows the answer, not the graph

**Decision:** No flight is offered until a solver has run. With a route, the camera flies the route, reconstructed from the delta stream: edges ordered by when each was first reached, direction inferred by chaining. With a field simulation it tours the network as before.

**Why:** the flight path was built at compile time from a depth-first walk, so it existed before there was any result, and it retraced its own steps at every dead end — on 806 streets that is a camera turning around every few seconds. The owner's objection was sharper than the symptom: flying over a network with nothing computed on it is a screensaver. A camera is for travelling a result.

Rebuilding the path from deltas rather than asking the solver for it keeps the layer boundary the contract draws: a solver emits `(entity_id, channel, t, value)` and nothing else, and the page derives a path from those alone. The deltas say when each edge was entered and never which way it was crossed, so direction comes from chaining — an edge whose source is not where the previous one ended is being travelled backwards.

**Consequence:** Measured on the same session: routing two points across Madrid gives a 3.24 km flight, where the old tour of that network was 114.87 km of doubling back; Net3's pulse still tours its 425 m. The fly row is hidden after compiling and appears only after a solve. Direction inference has no ground truth to check against — a route that revisits an edge in both directions would chain wrong — which costs nothing for a camera and would matter for anything measuring.

Found while testing, and left as #130: the demo pulse on a street network answers `the container declares no edge channel named 'flow'`. The picker offers three solvers with no sign of which network each one applies to.

## 2026-08-12 — A solver is offered only where its channel exists

**Decision:** The engine picker is built from the container. Each solver declares the channel it writes — `pressure` for EPANET, `on_route` for routing, `flow` for the pulse — and an option whose channel the file does not declare is disabled, saying what it would need. The first usable solver is selected, a line under the picker says what it will do, and a network that declares nothing keeps the row with the reason visible instead of hiding it.

**Why:** of the nine combinations of three examples and three solvers, three worked. The other six failed *after* the click, with `the container declares no edge channel named 'flow'` — a sentence about the format's internals offered to someone who wanted to see a network. Worse, the GeoJSON example could not reach any solver at all because the row disappeared, while the panel's own note promised the pulse for exactly that case.

The requirement is read off the file, not guessed from its extension. A solver writes into a declared channel, so a container that declares none is one it cannot touch, and that is a fact the file states rather than something the page infers. It also means a new importer that declares `on_route` gets routing offered with no change here.

**Consequence:** Verified across the matrix: Net3 offers EPANET and the pulse with routing disabled, Madrid offers routing with the other two disabled, and the GeoJSON offers none with a sentence saying why and `simular` disabled. Nothing fails after the click any more. A missing EPANET module — a build made without a WASI SDK — now says so instead of reporting `Failed to execute 'compile' on 'WebAssembly': HTTP status code is 404`.

Not in this change, and the owner's larger point: EPANET exposes none of its own options yet, and upstream/downstream tracing or isolation cuts are not EPANET options at all. They are graph work that belongs in the core and applies to any network, which is what would let one street network serve two domains without inventing hydraulics for it.

## 2026-08-12 — Reach, upstream, downstream and isochrone are one search, in the core

**Decision:** A new crate, `trama-trace`, answers what a network reaches. `Operation::Trace` takes seeds, a direction (`Forward`, `Backward`, `Both`), a cost (hops, length, seconds) and an optional budget; `Operation::Components` labels every edge with its connected component. `edge_lengths` moves into `trama-format`, where `trama-routing` had its own copy.

**Why:** the owner's fourth point, sharpened. Upstream/downstream tracing and isolation cuts are not EPANET options — they need no hydraulics, only topology, and putting them in the water solver would tie them to a `.inp` when their value is that they work on anything. They are also what lets one street network serve two domains honestly: a road has no diameter, but "what does this junction feed" and "what is cut off if I close this" are real questions about it.

They are one algorithm, not four. Direction, cost and budget are the three knobs; downstream is `Forward` with no budget, an isochrone is `Both` costed in seconds with one. Writing them separately would have meant the same loop four times under four domain names — which is precisely how a domain word enters a core that claims to have none. "Upstream" is water's word for `Direction::Backward`.

The forward case needs no direction rule at all: SPEC 4 gives a directed edge a single adjacency entry, at its source, so walking the CSR as written already refuses to cross an arrow backwards. Backward and Both are that list inverted and both lists together. This is the third time the CSR has turned out to encode a rule the caller expected to implement.

**Consequence:** Ten tests, two of them proved to bite by mutation — making `Forward` ignore direction fails `downstream_from_a_tail_reaches_nothing`, and disabling the budget check fails two more. Traces are emitted as a progression, so the scrub unwinds a spread the way it unwinds a route. Components answers "is this one network or twelve", which is the question that cost hours when the first OpenStreetMap extract came back in fragments and the map looked fine.

Moving `edge_lengths` to the core surfaced #132: those are Web Mercator metres, long by `1/cos(latitude)` — about 30% at Madrid's. `trama-routing` has always had it, so the playground's 3.24 km route is nearer 2.5 km on the ground. Fixed separately, because it changes published numbers.

## 2026-08-12 — Isolation, critical edges and allocation, from the same search

**Decision:** `trama-trace` gains three operations. `Isolation` removes a set of edges and reports what the seeds no longer reach. `Critical` finds the bridges — edges whose removal splits the network. `Allocation` labels each edge with whichever of several sources reaches it most cheaply.

**Why:** these are the questions a utility actually asks, and none of them needs a drop of hydraulics. "Close this valve, who loses water" and "close this street, what is cut off" are the same computation over the same graph; putting it in the water solver would have tied it to a `.inp` for no reason.

Two of the three are the search that was already there, read differently: isolation is the search with a blocked set and the answer inverted, allocation is one search per source keeping the cheapest arrival. Only bridges needed their own pass, and they get Tarjan's low-link on an explicit stack, because a city network is deeper than a recursive descent is willing to go. Parallel edges are handled by refusing to return along the *edge* just used rather than the node it came from: two pipes between the same pair are each other's spare and neither is critical.

**Consequence:** Sixteen tests. Mutation found something better than a weak test — flipping `>` to `>=` in the bridge condition fails as it should, but removing `blocked[edge] ||` from isolation failed nothing, because a blocked edge is never crossed and so never reached: the clause was dead. It is gone, and the comment now says why the result is right without it.

The ring-with-a-tail fixture is the shape that makes all three legible at once: cutting a ring edge loses only itself because the ring is its own spare, cutting the tail loses the tail, and two sources on opposite corners split the ring between them.

## 2026-08-12 — One small city, shipped compiled, answering questions that are not about water

**Decision:** `fixtures/madrid.osm.json` is replaced by `fixtures/teruel.trama`: the whole street network of Teruel, 2,770 nodes and 3,649 edges, shipped as a container rather than as source. The playground offers two more calculations over it — an isochrone from one clicked point, and the streets that are the only way through — and `trama-roads` declares the channels they write.

**Why:** the owner's third point. A slice of a large city is a demo; a whole small one is a network, and the questions asked of it are real. The honest part is which questions: a street has no diameter, so a hydraulic simulation over it would be a lie an engineer spots in ten seconds. What it does have is topology, and "how far do I get in ten minutes" and "which streets are the only access to something" are true questions with true answers — the same call a pipe network would make.

Shipped compiled because the numbers make the argument: 240 kB against the 1.9 MB of Overpass JSON, opening in under 300 ms with no compile step. Now that the page can open a `.trama`, the example is the first pillar demonstrated rather than described.

**Consequence:** Verified in the browser: opens in 292 ms, picker offers routing, reach and critical with EPANET and the pulse disabled, critical marks 685 of 3,649 streets, an isochrone from one point spreads with the scrub, and a route across town takes 28 minutes. The critical map is the one that argues by itself — the ring roads and the outlying spurs come back orange, the meshed centre stays blue.

Two things this surfaced. The renderer drew whichever edge channel came first, which was fine with one declared channel and wrong with four: each engine now names the channel it paints, which is not always the one it requires — EPANET needs `pressure` to prove the file came from a `.inp` and draws `flow`. And a real city has stray pieces: two clicks can land in different components, so `no route from node 1619 to node 1790` became a sentence about the network being in pieces rather than about node indices.

The published fixture has its own test — one component holding over 90%, crossable end to end, critical streets present but not everywhere. The first extract this project shipped was in fragments and rendered perfectly.

## 2026-08-12 — Metres on the ground, not metres on the projection

**Decision:** `edge_lengths` multiplies each segment by `cos(latitude)` at its midpoint, turning Web Mercator metres into ground metres. Everything costed by length — routes, travel times, isochrones, the odometer — changes with it.

**Why:** Mercator is conformal. It keeps angles by stretching distance, by exactly `1/cos(latitude)`: 31% at Teruel's 40.4°, 55% at 50°, unbounded towards the poles. `trama-routing` measured this way from its first commit and `trama-trace` inherited it, so every number the demo showed a visitor was long by a third. The reason it survived this long is that the factor is near-constant across one city and cancels in any comparison — which route is shorter was always right; how long it is never was.

The correction is per segment at its midpoint, exact to first order and wrong only where one segment spans degrees of latitude, which the tile grid rules out.

**Consequence:** The published example goes from 611.3 km of street to 465.8 km. A route across Teruel is 25 minutes rather than 28. The ten-minute isochrone *grows* — 5,214 deltas to 6,017 — because streets stopped costing a third more than they do. Critical edges are unchanged, being a property of shape and not of distance.

Correcting it also surfaced a test that was checking nothing: `trama-routing`'s suite kept its own copy of the length calculation, so it compared an uncorrected measurement against a corrected solver and failed. A test that measures differently from the code under test is comparing two answers and calling it agreement. It now calls `edge_lengths`.

The container is untouched: this is a reading of stored geometry, not a change to what is stored, so `fixtures/teruel.trama` is byte-identical and no file needs regenerating.

## 2026-08-12 — The same closure question, on water and on streets

**Decision:** The playground asks what loses service when something is closed. In this mode the first click says where service enters and the rest name the sections being closed, found by `nearestEdge` over the polylines the page already keeps. `trama-epanet` declares the topological channels alongside `pressure` and `flow`, so the question works on a pipe network and on a street network without either knowing about the other.

**Why:** it is the question a utility pays for — close this valve, who runs dry — and the one this project can answer in two domains with one call, which is the domain-agnostic claim stated where someone can press it. The engine has existed since the isolation work; what was missing was that the page could find the node under a click and not the edge.

Two clicks with different jobs in the same mode is a small interface risk, taken because the alternative is worse: inventing the source. A network's service point is a fact the operator has and the file does not, and guessing it would make the answer opaque exactly where it needs to be trusted. The row says which click is next rather than counting.

**Consequence:** Measured on the published fixtures. On Net3, 31 of 119 pipes are critical and **the worst single closure takes 25 of them out of service** — a fifth of the network behind one valve. On a meshed pipe the same closure loses only itself, which is what a ring main is for and is now a test. Teruel behaves the same way with streets.

This changed `fixtures/net3.trama`, compared byte for byte by the equivalence test: declaring channels changes the container. Regenerated deliberately. It also caught a test helper reading the channel section by assuming two dictionary strings per channel — three channels sharing the unit `"1"` contribute one entry between them, so the assumption walked off the end. It reads each record's own string id now.

## 2026-08-12 — Made publishable, not published

**Decision:** `@trama/core` gets a real manifest — a version, `exports` with subpaths, `files`, `sideEffects: false`, an entry point, and a README that is its npm page — plus a release workflow that publishes on a tag and builds `trama` for Linux, macOS x86 and arm, and Windows. Nothing is published: that needs an `NPM_TOKEN` and is the owner's decision.

**Why:** the package was `"private": true` at `0.0.0` and there were no binaries, so everything built here was reachable only by someone who clones the repository and has a Rust toolchain. A visitor could watch a city compile, take the container away and open it again, and still not install any of it. That is the ceiling on adoption, and it is a packaging problem rather than an engineering one.

Preparing without publishing is the split that matters. Publishing is irreversible — npm does not let a version be taken back — and it is an outward-facing act with the owner's name on it, so the work stops at the point where a tag would trigger it.

**Consequence:** `npm pack` produces 25.5 kB over 27 files, with the licence and README in it and no source maps or build info. The workflow refuses to publish when the tag disagrees with the manifest version, because a mismatch publishes a version nobody asked for and cannot be undone.

The fact worth advertising, found while writing this: nothing in `src` imports anything. Not `maplibre-gl`, not `fzstd`. The adapter describes the host map it needs with a type and decompression is a parameter, so **the published package has no runtime dependencies at all** — `fzstd` was in `dependencies` and only tests ever used it.

## 2026-08-13 — The sixty-second criterion, walked with a stopwatch

**Decision:** Phase 5's done criterion — *a stranger arrives, uploads their `.inp`, and sees their network simulated in under 60 seconds* — is measured by `engine/bench/journey.mjs` against the deployed site, not against a local build, and it is measured twice: once as the machine finds it, once through a throttled profile standing in for a mid-range phone on mobile data.

**Why:** the criterion had never been timed. It was plausible that it passed, which is exactly the state in which a launch gate quietly stops being one. Measuring the deployment rather than a local server is deliberate: a visitor waits for Cloudflare, for a cold cache and for the WASI module too, and a number that excludes them is answering a question nobody asked. The throttled run exists for the same reason the frame benchmark separates frame cost from frame cadence — an unthrottled desktop on fibre measures the desk it sits on.

**Consequence:** it passes with room to spare. Cold, no service worker, no HTTP cache: **1.6 s** to arrive, compile `Net3.inp`, run EPANET 2.3 over WASI and scrub time. Throttled to four times the CPU cost on a 1.6 Mbps link with 150 ms of latency: **6.3 s**. The criterion allows 60.

The interesting part is what the throttled run says about where the time goes. A 3,649-edge city takes 6.6 s against a 119-pipe network's 6.3 — three tenths of a second for thirty times the network. Compiling `Net3.inp` costs 47 ms on the desktop and 201 ms throttled; what a visitor actually waits for is 707 kB arriving, of which 399 are the compiler. **The playground's latency is a download, not a computation**, which means the lever that matters is the size of `trama_wasm_bg.wasm` and nothing about the solver or the format.

Two facts worth recording while they were visible. The page makes no third-party request at all: MapLibre is mounted on an empty style with a background colour (`site/demo/index.html:241`), so there is no basemap provider watching who opens which network — the privacy claim is structural, not a promise. And the harness asserts on the scrub actually moving, not on a slider becoming enabled, because a page that draws nothing is very fast.

## 2026-08-13 — WebGPU waits for MapLibre, and says so

**Decision:** The engine stays WebGL2-only. The WebGPU half of pillar two is deferred until MapLibre can hand a custom layer a WebGPU context, and this entry is the record that it was decided rather than forgotten.

**Why:** the renderer is mounted as a MapLibre custom layer, which is what buys us a basemap, a camera and a projection we do not maintain. MapLibre 6.2 hands that layer a WebGL2 context and only that one; its own typing says so — `contextType` is *"restricted to `'webgl2'`. This option is kept as a forward-looking API for future WebGPU support"*. Adopting WebGPU today therefore means leaving the custom-layer contract: our own canvas over the map, with the camera synchronised by hand every frame, and two renderers to keep in agreement.

That is a large, fragile change bought for performance we do not need. The frame benchmark draws 103,040 segments with animated state in 0.6 ms against a 16.7 ms budget, on integrated graphics from 2017. There is twenty times the headroom, so the honest reason to want WebGPU is compute shaders and a future workload, not this one.

**Consequence:** `KICKOFF.md` pillar two is met in part — GPU state textures, the temporal ring buffer, in-shader interpolation and the fly-through all exist — with the API choice left open. The cost of waiting is a dependency on someone else's roadmap; the cost of not waiting is owning a map camera. We chose the pillar we can revisit cheaply: only three of the twelve modules in `engine/src` name a graphics API at all — `line-renderer.ts`, `state-texture.ts` and the `maplibre.ts` adapter, 422 lines between them against 1,482 — so the day MapLibre offers the context, the port is small and local.

## 2026-08-13 — SQLite enters, but only through the command line

**Decision:** `trama export --to gpkg` writes a real GeoPackage, and `rusqlite` with its bundled SQLite is added to `trama-cli` and to nothing else. `trama-format` keeps its three dependencies and stays free of C.

**Why:** SPEC 9 has declared GeoPackage export mandatory since the format was written, and it was the one promise in the anti-lock-in claim with no implementation behind it. A `.gpkg` is a SQLite database with the tables OGC requires, so there is no version of this that avoids SQLite: hand-rolling the file format means writing b-trees to reimplement something that already exists correctly.

Where it lives matters more than which crate it is. `trama-format` compiles to WebAssembly for the browser, and a C database engine has no business riding along to compile a network in a page. Confining the writer to the command line keeps the wasm module at 399 kB and gives the dependency a boundary that a build failure will enforce if anyone moves it.

The export needed one thing from the format: coordinates in `EPSG:3857`, because the GeoJSON export ends in WGS 84 and GeoPackage layers store metres. Rather than un-projecting on the way out — a second implementation of arithmetic this repository has already been bitten by duplicating — `export` was split into `export` and `export_projected` over one shared reconstruction.

**Consequence:** Teruel exports to a 1.1 MB GeoPackage from a 234 kB container, with 3,649 edges, 2,770 nodes, 75 columns of OpenStreetMap tags and `roads:speed_ms` as a REAL. The cost of leaving is now a measured number rather than a promise.

Two properties were kept deliberately. `last_change` is written as the epoch instead of the wall clock, so the same container exports the same bytes and two people can diff their exports — GeoPackage requires the column, not that it be truthful. And absence survives: the fixture carries a string, an f64, an i64, a bool and, on two of three edges, the *lack* of some of them, which arrive as SQL NULL rather than as `0` or `""`, the distinction SPEC 5 insists on.

**Verified against GDAL**, which was the check the tests could not make: SQLite reading the database back is structural evidence, not interoperability evidence. GDAL 3.12 opens `teruel.gpkg` as two layers — 3,649 LineStrings and 2,770 Points — reads `EPSG:3857` from the file's own WKT rather than being told, types `roads:speed_ms` as a float and `osm:name` as text, and reports `osm:surface` present on 1,012 of 3,649 edges, so the nulls arrived as nulls.

It also handed back an unexpected confirmation. GDAL measures the street network at **611.3 km**, which is exactly the projected figure the 2026-08-12 entry recorded before correcting for Web Mercator distortion, against 465.8 km on the ground. An independent reader arriving at the same wrong number from the same geometry is the strongest evidence available that the file says precisely what this repository thinks it says — and a reminder that a GIS will happily add up projected metres and call them metres.

## 2026-08-13 — Polygons wait for v1, and the reason is the graph

**Decision:** v0 stays lines and points. Polygons are not added to the format now, and this is the record of the decision rather than of an oversight.

**Why:** the question polygons ask is not "can `GEOM` hold a mesh" — it can, and §3.3 already explains why the mesh fields exist and stay zero for lines. It is "what is a polygon in a graph". `PROP` and `STCH` both declare `entity_kind: 1=node, 2=edge`, so a pressure zone or a service area — the polygons this product would actually want — has nowhere to hang an ID, a property or a state channel. Adding a third kind means a spec change touching two sections, the compiler, the exporters and a triangle path in a renderer that has never drawn one.

That is the largest change available in v0, and nothing in the product asks for it today. `KICKOFF.md` already deferred everything that is not the network itself — labels, raster, complex symbology — to v1, and an area is in that family. The demo that would justify it, colouring a service area by what a closure does to it, does not exist yet; when it does, it will also say what the area needs to store, which is a better position to design from than this one.

**Consequence:** the README's "not yet" for polygons now points at a decision. When it is revisited, the shape to weigh is a third entity kind against a polygon as an existing entity's footprint — the first is a model change, the second reuses node identity and cannot express a zone that is not a node.

## 2026-08-13 — A CSV of points annotates a network; it is not one

**Decision:** `trama compile red.geojson red.trama --points meters.csv` reads KICKOFF's third v0 input. Each row becomes a `Point` feature carrying its other columns, joined to the node its coordinates land on. A CSV is never compiled alone, and a row that matches no node stops the compile.

**Why:** "CSV of points" had two readings and they are different products. A standalone cloud of points would need the format to accept a network with no edges — which the renderer cannot draw, since it builds ribbons from consecutive vertex pairs, and no solver can traverse. A network without topology is not a network, and every part of this format is built on that sentence.

The other reading is the one with a customer behind it: a utility has meters, sensors, elevations or customer counts in a spreadsheet, and a network somewhere else. Joining them by location is the ordinary shape of that work, and it is exactly what the format's `Point` already meant.

Reading the CSV needs no dependency. Quoting is the only rule RFC 4180 really has, and it is thirty lines: commas separate, quotes protect, a doubled quote inside quotes is one. Types come from the text — an integer, then a float, then `true`/`false`, then a string — and **an empty cell is an absent value, not an empty string**, because SPEC 5 keeps those apart and a spreadsheet is mostly blanks.

**Consequence, and the defect it uncovered:** a `Point` that matched no node used to be dropped in silence. Compiling three features produced two entities, reported success, and lost the third — verified before the fix on a file with a point 5 km off the network. That is the worst of the three available behaviours, worse than refusing and much worse than warning, because nothing downstream can tell that anything is missing.

It is now an error naming the coordinates the author wrote, not the projected cell they have never seen. The join is on the SPEC 4.2 quantization cell — about 4 cm — so a meter surveyed three metres from its junction does not match, and says so. Snapping within a tolerance is a real feature and a separate decision; guessing which node the author meant is not something a compiler should do by default.

This is a behaviour change for anyone whose GeoJSON carries points that sit on nothing: hydrants, labels, points of interest. They now get an error instead of a map. That is the right trade for a format whose entire claim is that a file says what it contains, and it is consistent with the rest of the writer, which already refuses duplicate IDs and non-boolean `_trama_directed` rather than interpreting them.

## 2026-08-13 — Vector tiles, written by hand

**Decision:** `trama export red.trama tiles/ --to mvt` writes one `{z}/{x}/{y}.mvt` per `GEOM` record, encoding the protobuf directly rather than generating it from the Mapbox schema.

**Why:** MVT is the exit that needs no TRAMA at the other end. GeoJSON and GeoPackage hand the data to a GIS; a tile pyramid hands the *map* to any MapLibre or Mapbox client already deployed, with nobody installing anything of ours. That is the anti-lock-in claim stated in a stack we do not control, and until now it was a line in the README with no code behind it — the same shape of gap GeoPackage had that morning.

Writing the protobuf by hand is the smaller of the two options. The slice MVT uses is varints, length-delimited fields and packed `uint32` arrays; a generator would add a build-time dependency and a schema file to emit four message types. It is about sixty lines, and the parts with a real chance of being wrong — varint growth, zigzag, the command integers, the extent conversion — are unit-tested against values from the specification.

The tile-local geometry was already in the file: SPEC 3.2's `Path` record names its `edge_index`, so a tile knows which edges it holds without consulting anything. That reading moved into `trama-format` as `parse_tile` rather than being written a second time in the command line, which is the mistake this repository has already paid for once with length arithmetic.

**Consequence:** Teruel becomes 107 tiles, 604 kB against the container's 234 kB and the GeoPackage's 1.1 MB. Verified by decoding the whole pyramid with an unrelated MVT reader and reassembling it: all 3,649 edges and all 2,770 nodes present, and across 21,230 tile vertices **no vertex moved more than 0.32 m** from what the container says. That number is the format's own quantization meeting MVT's: 4,096 units per tile against the 65,535 SPEC 3.1 stores, so half a unit at `z14` is about 30 cm, and the measurement lands exactly where the arithmetic says it should.

What does not survive is in SPEC 9 and is not small: CSR topology, traversal order across tiles, nullable typing, channel declarations. A tile is a picture of a network, not a network — which is why MVT is export-only and always will be.

## 2026-08-14 — SWMM arrives as a third domain, engine-less on purpose

**Decision:** `core/trama-swmm` imports and exports a SWMM drainage network — junctions, outfalls, storage and dividers as nodes; conduits, pumps, orifices, weirs and outlets as edges; `[XSECTIONS]` folded into the link it shapes; everything hydrological carried unread in an `XTRA` record owned by `swmm`. The engine is deliberately not in this step: no `swmm-sys` exists on crates.io, so binding the C toolkit is its own decision with its own options, and the importer is useful without it — a drainage network displays, traces and answers "what stops draining if I close this" today.

**Why SWMM next:** it is the cross-sell vertical. The market analysis of 2026-08-13 put it first among the candidate solvers because the buyer already exists — the same utility that runs the water network runs the sewers, and the incumbents sell the two as a suite. It is also the cheapest possible test of the multi-domain claim, because SWMM's `.inp` shares EPANET's text shape exactly: bracketed sections, whitespace fields, `;` comments, `[COORDINATES]` and `[VERTICES]` spelled the same.

That sharing set the two structural decisions. `trama-swmm` depends on `trama-epanet` with default features off, for `inp` and `Reprojection` only — a dependency between domain crates is unusual layering, but the alternative was copying 130 lines that would then drift, and the C engine stays behind the feature gate the dependency never enables. If a third `.inp`-family crate appears, the shared modules move to their own crate; the Cargo.toml says so.

And the suffix collision is resolved by refusal rather than guessing. `.inp` belongs to the EPANET importer, which was here first; SWMM claims no suffix and is reached by `--importer swmm`. Each importer recognises the other's sections — `[PIPES]`/`[RESERVOIRS]` on one side, `[CONDUITS]`/`[SUBCATCHMENTS]` on the other — and redirects by name instead of parsing to an empty network, which is the silent-loss shape this repository keeps finding and keeps closing. `--to inp` needs no flag at all: the container says which dialect it came from through the `XTRA` owner only the right exporter recognises.

**Consequence:** the pyswmm test network (BSD-2, attributed in `tests/networks/README.md`) makes the round trip with every entity, every property and the hydrology byte-identical. Channels declare `depth` and `flow` in the file's own units plus the three topological ones, so a drainage network answers isolation and criticality questions with the solver that already exists. What a container cannot do yet is *run*: verification by simulation, the standard the EPANET round trip is held to, waits for the engine decision.

## 2026-08-14 — SWMM runs: the -sys crate we had to write ourselves

**Decision:** `trama-swmm-sys` vendors EPA SWMM 5.2.4 verbatim — `src/solver` only, 1.8 MB of public-domain C — and compiles it with `cc`, no CMake, no OpenMP. `trama-swmm` gains a `solver` feature in the image of `trama-epanet`'s: the container is exported back to a `.inp`, the toolkit steps through it, and node `depth` and link `flow` come out as packed deltas, served by `trama-solver-swmm` over HTTP + SSE on 8805.

**Why this shape:** EPANET had `epanet-sys` on crates.io; SWMM had nothing, so the binding is ours to carry. Vendoring beats downloading at build time (a network build step is a flaky build step) and beats CMake (the engine is C99 with no configuration; a second build system is a tool for CI to be missing). OpenMP stays off because every use is guarded and a deterministic single thread is the right default for a solver whose output feeds byte-compared tests.

Three API differences from EPANET mattered. The 5.2 API is a single global project, so a process cannot run two simulations at once — a mutex in the solver makes the library tell the truth under any caller. Elapsed time arrives in decimal days, and the loop advances with `swmm_stride` at the file's own report step rather than the routing step, which would have multiplied the deltas by orders of magnitude without changing what a scrub shows. And indices are zero-based where EPANET's are one-based, which is exactly the kind of off-by-one a test that compares two runs of the same network catches for free.

**Consequence:** the round trip now meets the same standard as EPANET's — defined by simulation, not by bytes. The fixture's 58 hours simulate identically from the same container twice, and the exported `.inp` agrees with the original per entity, per channel, per timestep. The first delta the server ever streamed was a junction reporting depth in feet at t = 3600 s, which is three claims — the graph, the units, the clock — agreeing in fourteen bytes.

What is deliberately not here: WASI. The build needs the same shim treatment EPANET's got in `core/wasi/`, and the playground needs a second module and an engine choice. That is its own change, on a crate that is now ready for it.

## 2026-08-14 — SWMM reaches the browser through the door EPANET opened

**Decision:** the SWMM solver ships as a second WASI command, `trama-swmm-wasi`, built by the same `core/wasi/build.sh`; `trama-wasm` gains `compile_swmm`; and the playground compiles a SWMM `.inp` and simulates it in the page, no server involved.

**Why it was cheap, and why that is the point:** the entire integration reuses decisions already paid for. The `mkstemp` shim EPANET needed turns out to be the only libc gap SWMM has too — one flag in `trama-swmm-sys`'s build script for the WASI target. The page's WASI runner was EPANET-shaped in name only; its module name became a parameter and both toolkits go through it. And the suffix dance is the compiler's own redirect message driving a retry: the page tries EPANET, and when the error names SWMM it compiles as SWMM — the browser equivalent of `--importer swmm`, decided by the file rather than by the user.

**Consequence:** verified end to end with a real browser before merging: the pyswmm fixture compiles in 35 ms, the page selects SWMM by itself — EPANET is offered disabled because the container declares no `pressure`, which is the channel system doing its job — and simulating returns 10,080 deltas: 24 hours at the file's own 60-second report step times seven entities, exactly. The WASI module also ran under Node's own WASI against the full 58-hour window as a second, browser-free witness.

The playground now holds the whole commercial claim in one page: water, drainage, and streets, compiled and solved locally, on three solvers that share one contract and a core that cannot tell them apart.

## 2026-08-14 — The launch waits for depth, and depth starts with water age

**Decision (owner):** the public launch is deferred. Not for polish — the criterion it was gated on passes with a fifty-fold margin — but for substance: three solvers that answer one question each is a demo, and the launch should be a product. The work turns inward: more operations on the solvers that exist, then more solvers.

**First deepening: water age.** `EN_AGE` was already in the linked toolkit, unused. The importer now declares an `age` node channel in hours, and the solver runs EPANET's quality pass after its hydraulic one — switched on programmatically with `EN_setqualtype`, so the user's `.inp` needs no `[QUALITY]` section, and advanced with `EN_nextQ`, which steps by hydraulic events and keeps age on the same cadence pressure reports at.

**Why age first:** it is the highest ratio of question-answered to code-written available. How long water sits in the network is what chlorine decay, taste complaints and stagnation regulation are all about, and no tool shows it to a small utility. The physics is now a test: age starts at zero, the day's mean grows, and the worst value stays under 25 — hours, not seconds, which is the kind of unit mistake a channel declaration exists to prevent.

**The compatibility rule that fell out of it:** age is an offer, not a demand. The solver writes it only where the container declares it, so every `.trama` compiled before this change solves exactly as it did. `fixtures/net3.trama` regenerates because declaring a channel changes the container — the 2026-08-12 precedent — and the equivalence suite re-verified identity across both compilers.

Chlorine decay and source tracing are the same loop with a different `EN_setqualtype` argument, and are recorded here as the natural next two.

## 2026-08-14 — Flooding, by the same rule age set

**Decision:** the SWMM importer declares a `flooding` node channel in the file's flow units, and the solver reads `swmm_NODE_OVERFLOW` in the loop it already runs — one more `getValue` per node per report step, written only where the container declares the channel.

**Why:** where the system floods is the question stormwater modelling exists to answer, and it was one enum constant away. The channel is zero almost everywhere almost always, which is exactly why the exceptions matter and why a scrub over it reads instantly: the map lights up where and when the network gives up.

**Consequence:** the offer-not-demand rule set by EPANET's age channel is now a convention rather than a case — two solvers follow it, and old containers keep solving untouched by either. The test demanded more than shape: the fixture's storm must actually overwhelm something (`peak > 0`), because a flooding channel no fixture can light is untestable and the storm decorative. It does.

## 2026-08-14 — Close it and see: the scenario, answered with physics

**Decision:** both EPA solvers accept `closed_edges` — stable entity ids, carried as strings because a `u64` does not survive a JSON number. EPANET expresses the closure as a `[STATUS] <name> Closed` section appended to the rebuilt `.inp`; SWMM omits the closed links from its rebuilt file entirely, along with their cross-sections and vertices.

**Why two mechanisms for one parameter:** each engine gets the closure in the only language it speaks. EPANET has a first-class construct for a closed link — a shut valve still exists, holds pressure against its faces, and appears in results. SWMM has neither a status column for conduits nor a runtime API that will touch one (`setLinkSetting` returns without acting on `CONDUIT`; read in the vendored source, not the manual), so a blocked culvert is expressed the way the engine can hear: absent from the network. The asymmetry is honest — it mirrors what closing means physically in each domain — and invisible to the caller, who names edges by the same stable ids the deltas use.

Because both solvers rebuild the `.inp` from the container on every run, the scenario needed no new state anywhere: it is a text edit between export and simulation, which is the kind of feature the export-then-simulate architecture was quietly buying all along.

**Consequence:** the physics is tested, not asserted. Closing Net1's pipe 10 — the single link between the pump and everything else — must change more than ten reported values, and it does. Blocking the fixture's only conduit into storage removes exactly that link from the results and changes the rest. An id that names no edge stops the run with the id in the message, rather than becoming a line EPANET would refuse later or SWMM would silently fail to match.

The playground follow-up: let the click flow that already picks cut edges for the topological solver hand the same edges to the physical ones.

## 2026-08-14 — The scenario reaches the page

**Decision:** the playground's click flow feeds the physical solvers. With EPANET or SWMM chosen, clicking edges marks closures — the same red markers the topological solver uses — and simulating passes their stable ids through the WASI argv. Zero closures simulates the network as it stands, so the scenario is an offer on top of the existing flow rather than a mode.

**Consequence:** the closure question now has two answers on one page, and the distinction teaches the product. "Qué se queda sin servicio" answers with topology in milliseconds; EPANET answers with physics in seconds — and the physics knows things the topology cannot: a closed valve drops pressures on paths that remain connected. Verified through the exact binary the browser runs: closing one Net3 pipe changed 8,033 of 8,451 reported values, and the closed run emitted more deltas than the open one, because EPANET inserts hydraulic events when the network changes regime. An id naming no edge refuses by name through the same argv, also verified.

One structural correction fell out: `closed_edges` parsing moved from `trama-epanet` to `trama-solver`, because SWMM's WASI build — which does not enable EPANET's solver feature — needed it, and because it was never EPANET's: any solver simulating a network can be asked to run it with edges closed. The contract crate is where contract conventions live.

## 2026-08-14 — The other two quality analyses, and the manifest that could not name them

**Decision:** the EPANET solver runs all three EN_QUALITY analyses over one set of saved hydraulics. The chemical the file's `[OPTIONS] Quality` names becomes `chem:<name>` in the file's own unit — simulated under its own `[QUALITY]`, `[SOURCES]` and `[REACTIONS]`, which travel through XTRA, with no setting of this solver's; `EN_setqualtype` is not even called for it. Every reservoir becomes `trace:<name>` in percent via `EN_TRACE`, offered to files that declare nothing, because tracing asks nothing of them. Age keeps its pass and moves last, since `EN_setqualtype` leaves the project on whatever ran before it. All three are offers in the established sense: written where the container declares them, silent elsewhere.

**What it forced — contract 0.3.0:** a manifest output had to name its channel literally, and these channels are named by the input: `chem:chlorine` in one network, `chem:tce` in another, one trace per reservoir. `channel_prefix` (SOLVER_CONTRACT 2.2) resolves an output to every `STCH` declaration bearing the prefix — possibly none — with entity kind and unit validated per resolved declaration exactly as before. The empty prefix is malformed for the same reason a unit wildcard is. Amended first, in its own PR, per the spec-leads-the-code rule.

**Consequence:** verified through the exact binary the browser runs — Node/WASI over the regenerated net3 fixture reports 13,689 deltas across five channels, the two traces spanning 0..100 and clamped there, because percent is bounded by meaning rather than by the toolkit's numerics. The new channels also surfaced a latent test bug: the rebuilt-network comparison keyed values by channel id, and the exporter reorders reservoirs, so the same channel recompiles under a different id. It now keys by channel name, which is the identity that actually survives a round trip.

The natural next: the playground offers `chem:` and `trace:` channels dynamically — today its engine list only knows fixed names.

## 2026-08-14 — Node channels reach the map as gradients

**Decision:** the line renderer paints node channels. Each instance now carries its edge's source and target node indices and its segment's arc-length fraction along the edge; told a channel's entity kind is node, the fragment shader blends the two endpoint texels along the line — the source→target gradient EPANET's own GUI uses for quality. The `StateRing` texture already held every node row; what was missing was only the shader's way to address them. The lookup arrives as an optional `edgeEndpoints` on the layer, so existing callers change nothing and an edge the caller cannot place falls back to node zero.

**Why a gradient and not a dot per node:** the page renders lines and only lines — SPEC 3.3 has no node primitive — and the physics genuinely varies along a pipe: pressure drops from one junction to the next, chlorine decays in transit. The gradient is not a workaround; it is the honest picture. One ceiling, marked in the code: arc fractions are per tile-local path, so an edge clipped by a tile border restarts its gradient at the seam. The EPA networks the feature exists for fit inside one tile; carry a path's starting fraction in GEOM if the seam ever shows.

**The playground grows a layer picker** fed by measurement, not by declaration alone: after a run it offers exactly the channels the delta stream wrote, labelled in Spanish with the file's own units — presión (psi), edad del agua (h), % desde Lake — and a channel with no declared range gets the run's own maximum as its ramp, so a 130 psi network and a 30 psi one both use the whole scale. Switching layers re-mounts the MapLibre layer over the same ring: no re-solve, no re-fetch.

**Consequence:** verified with Playwright over a local build — Net3 solves and offers five layers; pressure paints visible gradients with a hot spot at the pump; % desde Lake is uniformly zero at t=0, which is not a bug but the physics (no lake water has arrived yet), and moves with the scrub. Zero page errors. What #166 put in the binary, a visitor can now see.

## 2026-08-15 — A substation is one point on the map and two nodes in the network

**Decision:** `trama-power` imports a pandapower network — buses as nodes, lines and transformers as edges, every column as a `power:` property, the rest of the tables in one `XTRA` record. When two buses are drawn at the same coordinate, the importer separates them by ten metres eastward, deterministically, first-one-stays.

**Why the separation exists:** SPEC 4.2 identifies a node by where it is, which is right for a pipe network and wrong for a substation. pandapower's own `mv_oberrhein` draws each transformer's 110 kV and 20 kV buses at one coordinate, because that is the truth — they are in the same building. Compiled as they arrive, the two collapse into one node, and a collapsed transformer is a short circuit: the network loses a voltage level and every load flow over it is wrong. The failure is silent, which is what makes it worth a mechanism rather than a note. Ten metres is below what a network-scale view resolves and far above the quantization floor, so the map is unchanged and the graph is correct. This is the same kind of translation `trama-roads` does for `oneway`: domain knowledge the core must never learn.

**What is in `XTRA` and why it is not a duplicate:** the loads, generators, external grid, switches and standard types travel whole, as EPANET's non-entity sections do. The three expressed tables keep their column order and lose their rows — the schema is what a reader needs to put the rows back, and the rows are in `GRPH` and `PROP` already, so SPEC 330 holds.

**Neither channel declares a range.** A bus under 0.9 p.u. and a line over 100% loading are the two answers an operator runs the study to find. A declared range would have the host reject exactly those deltas as invalid: the failure state is not an invalid state.

**Consequence:** 283 kB of pandapower JSON compile to a 48 kB container that `trama validate` accepts, exporting 179 nodes and 183 edges — the same counts the source has, with both transformer sides intact. The Python solver that reads this container back over the HTTP contract is the follow-up, and is the part that will prove the format is legible outside Rust.

## 2026-08-15 — The format is open because someone else read it

**Decision:** `solvers/pandapower` is a Python package, in a new top-level `solvers/` directory, holding a reader for the TRAMA format written from `docs/SPEC.md` alone — header, section directory, CRC-32C, zstd frames, `GRPH` with its LEB128 identity blocks, `PROP` with its dictionaries and presence bitmaps, `STCH`, `XTRA` — with nothing imported from `trama-format`, plus a server implementing SOLVER_CONTRACT section 6.

**Why it is worth its weight:** every solver until now was a Rust crate linking the reference implementation, so nothing had ever tested the claim the project is built on. "Open format" is not a licence and not a published document; it is the property that a second implementer can decode a file without the first implementer's code. That property is either demonstrated or assumed, and it was assumed.

Writing the reader found three things the specification had never said: the width of a string's length prefix (`u32`), the bit order within a presence bitmap (least significant first, so a reasonable reader could have attributed every property to the wrong entity and still decoded without error), and the layout of column values per `value_type`. All three are now in SPEC 0.4.1. This is the return on the exercise, and it arrived exactly where it was expected to: not in the parts under test, but in the parts only one implementation had ever exercised.

**No web framework**, following the Rust runtime's own reasoning: a solver is a plugin, not the product's backend. `http.server` and a loop means running this needs pandapower and nothing else — the owner's FastAPI constraint is for a backend, and this is not one.

**A load flow is one instant**, so the solver writes one by default rather than inventing a daily curve. `load_scaling` turns it into a series — one real load flow per multiplier, spread across the interval, never an interpolation between two. A solver that supplied a demand profile of its own would be reporting a modelling assumption as a measurement.

**The access policy is documented because section 6 requires it**: HTTPS only, no credentials, 64 MB cap, `sha256` verified before parsing, and `--allow-http` restricted to loopback so a development convenience cannot become a way into an intranet.

**Consequence:** the network survives the round trip to the resolution an `f32` delta has — worst deviation 5.8e-8 p.u. on bus voltage and 2.7e-6 % on transformer loading, against `pandapower.networks.mv_oberrhein()` solved directly. 11 tests, including the full client path: fetch over HTTP, solve, read `ready`/`delta`/`complete` back. `solver-checks.yml` now gates it.

**One thing to hold onto:** the independence of that reader is the whole point of it. A decoding bug there must never be fixed by copying what the Rust does. Read the spec; when the spec is silent, amend the spec.

## 2026-08-15 — The page talks to a solver it cannot run

**Decision:** the playground compiles pandapower networks in the browser (`compile_power` in `trama-wasm`, routed by the `_module` signature the document writes about itself rather than by a suffix three formats share) and solves the shipped grid through the **server** runtime of the contract — the Python solver, over HTTP and Server-Sent Events, using the SSE client the engine already had and nothing else.

**Why it matters more than a fourth example:** every solver the page had ever run was WebAssembly it loaded itself. This one it cannot run at all — it is Python, on NumPy and SciPy — so the page does what any other client would: posts a request and reads deltas back. Contract section 5 says "a client MUST NOT need to know which runtime produced a result", and until now that was a sentence rather than a demonstration. The layer picker, the ring buffer, the scrub and the shader treat those deltas exactly as they treat EPANET's.

**What it costs, stated on the page rather than hidden:** a server solver is handed a URL, not bytes, so it can only solve a container it can reach. A shipped example is reachable; a file the visitor dropped never left their machine and has none. The page says exactly that when asked, because it is the privacy promise working, not a gap to paper over.

**The demand curve lives in the caller.** The page sends 24 hourly multipliers and the solver runs 24 real load flows — 8,688 deltas — rather than the solver inventing a profile. A solver that supplied its own would be reporting a modelling assumption as a measurement.

**A colour bug the electrical domain exposed:** the ramp for a channel with no declared range ran from zero to the measured maximum. That reads well for a flow and hides a voltage entirely — bus voltage lives between 0.97 and 1.03 p.u., the last 3% of a scale beginning at 0, so a healthy network and a collapsing one painted the same colour. The ramp now covers the measured span, low end included, and the page prints both ends with the unit beside the picker. Every channel added this week was legible only by luck until this.

**Consequence:** verified with Playwright against a local build and a running solver — the grid loads, offers only pandapower (the only engine whose channel it declares), solves through the HTTP contract, paints voltage and loading, and moves with the daily curve, with no page errors. Chrome's Private Network Access preflight needed answering for a public page to reach a loopback solver; the header is sent only when the operator started the solver with `--allow-http`.

## 2026-08-15 — Fire flow, and the race it uncovered

**Decision:** the EPANET solver rates fire flow — the largest demand a named node sustains while the network still holds a residual pressure, defaulting to 20 psi or its metric equivalent depending on what `[OPTIONS] Units` chose. It is its own function rather than another parameter of `solve`, because it is a different kind of thing: every other channel is a reading of one simulation, and this is a search over dozens. Bracket by doubling, then bisect; the added demand rides in a `[DEMANDS]` section appended to the rebuilt `.inp`, the same export-then-simulate seam the closures use, so no new toolkit call was needed. It composes with `closed_edges` by construction, because "what can this hydrant give while that main is shut" is the question a real study asks.

**Why it is worth having:** it is the only question a utility is asked *in writing*, by fire departments and insurers, and the only one none of the other channels can answer. A hydrant's rating is not a property of the pipe it hangs off; it is a property of the whole network on the day, which is precisely why it has to be modelled.

**`entity_list` replaces `closed_edges` as the convention.** Two parameters now name entities, so the contract crate carries the rule — ids as strings, absent means empty — rather than one parameter's helper. `closed_edges` is now a call to it.

**The bug this found, which was older than this feature:** running dozens of solves in one test binary turned the suite red at random, with EPANET refusing perfectly good networks with error 200. `EN_createproject` looks like it makes the engine reentrant and does not — enough state is still global that two projects open at once collide. It had been latent since the first solver: every caller until now ran one simulation at a time, so the window never opened. The engine is now serialised behind a mutex, which is the shape `trama-swmm` already took for the same reason. Verified by running the suite six times rather than once, because a race that passes once has proved nothing.

**Consequence:** the physics is tested, not recorded — more available flow at a lower threshold than a higher one, two hydrants at opposite ends of Net3 rated differently, and a closed main leaving the hydrant downstream with less. A node already below the threshold rates zero rather than some positive figure, which would be the most dangerous kind of wrong answer.

## 2026-08-15 — rustpower evaluated and set aside, with the measurements

**Decision:** we do not adopt `rustpower` as the browser-side power flow today. Recorded here with the evidence so nobody spends the afternoon again.

`rustpower` (MPL-2.0) looked like it would turn "write a Newton-Raphson" into "package a crate": it is Rust, it reads pandapower JSON natively — which is what our `XTRA` already carries — and it **does compile to `wasm32-unknown-unknown`** with its default features, which was the first thing checked and the reason it was worth trying at all.

**Where it works, it is exactly right.** On pandapower's `case9` it converged in two iterations and reported bus voltages of 0.95762–1.00338 p.u. — the same numbers pandapower gives, to every digit printed. The mathematics is not in question.

**Where it stops:**

1. **It targets pandapower 2.x.** Loading a 3.x document fails on absent columns (`tap_phase_shifter`, `const_i_percent`, `const_z_percent`) and, on the MATPOWER-derived cases, on a column pandapower now writes as a float where the deserializer wants an `i32`.
2. **It assumes bus indices are contiguous.** `mv_oberrhein` numbers its buses 0–8, 29, 30, … and the loader panics unwrapping a lookup. Fixable on our side — renumbering is the kind of translation our importer already does — but it has to be done.
3. **It does not converge on a real distribution network.** With indices made contiguous, `mv_oberrhein` runs 100 iterations and lands on voltages between 0.00002 and 1.3 p.u. while pandapower solves it comfortably. Stripping the switches and the distributed generation does not help: still no convergence, still absurd voltages.

**The pattern in those measurements:** `case9` has no transformers and one voltage level. `mv_oberrhein` has two transformers and two levels, 20 kV and 110 kV — and *every* distribution network does, which is the entire use case. The evidence points at transformer handling or the per-level voltage base rather than at anything we could feed it differently.

**What this leaves.** The market finding stands: nobody has put a power flow in the browser as a product, and "the network never leaves your machine" is unclaimed in electrical. The route there is just longer than packaging someone else's crate — either debugging a young engine we do not own, or writing the solver, where the compensation is that pandapower is right here as an oracle to test against. Neither is today's work, and the honest interim is what the page already says: this one calculation runs on a server, and we are working on it not having to.

## 2026-08-16 — The licence names a range, because a release is not a licence event

**Decision:** the BSL parameters name `TRAMA source code, versions 0.x` rather than a single version, and the Change Date moves from 2030-08-09 to 2030-12-31. The 2026-08-09 entry above stands as written: it records what was decided then, and this supersedes it rather than rewriting it.

**What it fixes, found while preparing the first release:** `LICENSE` named `0.0.0-pre-alpha` while `engine/package.json` and the Rust workspace both declare `0.1.0`. BSL 1.1 licenses the work it *names*, so publishing 0.1.0 under that text would have shipped the one artefact anybody downloads without an explicit grant covering it. Nobody would have noticed until it mattered, which is the kind of gap that only surfaces in a dispute.

**Why a range instead of the exact version:** naming the exact version is more precise and costs a licence PR on every release — and the failure mode of forgetting one is exactly the ambiguity we just found, arriving silently. A range has one edit and no recurring obligation. `0.x` is identifiable, which is all BSL 1.1 asks of the parameter, and 1.0 is the natural place to reparameterise: a stable release deserves its own Change Date rather than inheriting one set before the format existed.

**Why the date moves:** BSL 1.1 caps the Change Date at four years after the Licensed Work is first publicly distributed. Publishing on 2026-08-16 left 2030-08-09 compliant by seven days — so a fortnight's slip in the release would have put the parameter out of bounds with the tag already public. 2030-12-31 is a round date with room, and still well under the cap. It is a ceiling, not a floor: nothing in the licence objects to converting sooner than four years.

**Consequence:** every 0.x release is covered by one grant that converts on one date. `README.md` says so in the same words. The next time these parameters need attention is 1.0.

## 2026-08-16 — The power flow is ours, and pandapower is the oracle it is measured against

**Decision:** `trama-power` solves. An AC power flow by Newton-Raphson in polar coordinates lives in `flow.rs`, the pandapower-to-electricity translation in `network.rs`, and the contract implementation in `solver.rs`, reached as `trama-solver-power` over HTTP+SSE and as `solve_power` from `trama-wasm`. The decision of 2026-08-15 not to adopt `rustpower` stands; this is the other route that entry named, and the compensation it promised — pandapower as an oracle, right here — is what made it a day's work rather than a research project.

**Why it was worth doing at all:** the electrical domain was the only one whose calculation needed a server, which meant the one promise the product is built on — the network never leaves your machine — had a hole in it exactly where the market opening was. Every other engine either compiles to WASI or is Rust already.

**Scope, chosen rather than defaulted:** slack and PQ buses, lines in π, transformers with tap and phase shift, static generation as negative load, and open switches. That is a distribution network. PV buses and reactive limits are transmission's, and the Jacobian is already blocked for them, so adding them changes the assembly and not the structure.

**The oracle is the design, not a checkpoint at the end.** A load flow that converges is not one that is right — the `rustpower` entry above records an engine that converged on `mv_oberrhein` and reported voltages between 0.00002 and 1.3 p.u. So no formula here was written from a textbook and hoped over: each was checked against the matrix pandapower itself builds. That found four things a derivation would not have:

1. **Transformers are modelled in T, not π.** pandapower's default `trafo_model="t"` puts the magnetising branch between the two leakage halves and converts star to delta. Using the π directly changes the fifth digit of every transformer impedance — far too little to look wrong, and far too much to match.
2. **An open switch does not delete its branch.** pandapower moves that end onto a bus of its own, so the line still charges its capacitance from the connected side. Deleting it instead reports zero loading where the reference reports 0.04% to 0.32% — six branches quietly wrong in a way no aggregate check would catch, which is why they have a test of their own.
3. **The flat start does not survive a Dyn transformer.** This is what actually broke the first working version: parameters correct to twelve digits, topology correct, injections correct, and a residual of 957 p.u. growing instead of shrinking. A 150° shift puts the answer near −150° and Newton-Raphson started at zero is on the wrong side of a basin it cannot cross. Walking the shifts out from each slack before iterating fixes it, and every distribution network has that transformer at its head.
4. **Iron losses ride in a conductance the π model is usually written without.** The half of it at the tapped end is divided by the tap squared, which is where the last discrepancy in the admittance matrix turned out to be.

**Consequence:** 179 bus voltages within 1e-8 p.u. and 183 branch loadings within 1e-6 percentage points of `pandapower.runpp` on the very file in `tests/networks/`. 21 tests. The failure path is as deliberate as the success path: a network that will not solve names the bus with the largest residual, because "it did not converge" leaves an operator a whole network to search, and a branch whose source declared no rating writes no loading delta at all rather than a NaN that would poison the colour ramp of every other branch on the map.

**`solvers/pandapower` stays exactly where it is.** It is not superseded by this and must not be removed: it is the second implementation, in another language, that reads the container from `docs/SPEC.md` alone, and it is the oracle these tests are measured against. Deleting it would remove both the evidence the format is open and the only thing that would notice if this solver drifted.

## 2026-08-16 — The short circuit, and the fixture that could not answer it

**Decision:** `trama-power` runs a second study. `network::Study` picks what the same file becomes — a load flow, or IEC 60909's maximum short circuit — and the solver reaches it through `study = "fault"`, writing a `fault_current` channel in kA. `solve_fault` exposes it to the browser.

**Why this one next:** it is the second question a distribution utility answers in writing. A load flow says what the network is doing; a short circuit says what it would do at its worst, and that number is what sizes every breaker and sets what a relay must survive. It is to an electrical network what the fire flow of 2026-08-15 is to a water one — and, like that one, it is not a reading of the study already implemented but a different calculation over the same import.

**Two networks out of one file.** A fault is not a load flow with different numbers. IEC 60909 drops the load entirely, drops line charging and transformer iron losses, ignores the phase shift because a magnitude cannot see it, corrects each transformer's impedance by a factor the standard prescribes, and replaces the slack with the source impedance the infeed declares. Expressing that as a `Study` parameter on one importer rather than a second importer is what keeps the two honest: the same switches, the same auxiliary buses, the same topology, so a fix to one cannot silently pass the other by. `tests/fault.rs` asserts that the load flow still agrees on the new network for exactly this reason.

**Every constant checked against the reference, again.** As with the load flow, nothing here was derived and hoped over: the external grid's impedance (`c·S_base/S_sc`, split by R/X), the transformer correction factor (`0.95·c_max/(1+0.6·x_T)` = 0.97481406 on the fixture, to eight digits), and the absence of charging in this mode were all read off pandapower's own matrix before being written.

**The fixture had to change, and that is the finding worth recording.** `mv_oberrhein` cannot be faulted: its external grids declare no `s_sc_max_mva`, so pandapower fails with a NaN in its own admittance matrix. That is not a gap in the network — it is what a pandapower file usually looks like, because the fault level is data the load flow never needs. So the container inherits the same limit, and the solver says which column is missing rather than assuming an infinite bus, which would report a fault current limited only by the network's own lines and read as a perfectly plausible number to whoever was sizing a breaker from it. `cigre-mv.json`, CIGRE's medium-voltage benchmark as pandapower ships it, is the fixture that does carry the level.

**Consequence:** every one of 15 bus fault currents within 1e-9 relative of `pandapower.shortcircuit.calc_sc`, spanning 1.2 kA to 26.2 kA. 27 tests in the crate. One bug found by writing the tests rather than the code: the fault path was reading `voltage_channel` to name its output, so a caller who passed both would have had fault currents written into the voltage channel — the kind of mistake that produces a map rather than an error.
