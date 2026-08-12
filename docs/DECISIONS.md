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
