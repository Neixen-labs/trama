# EPANET import and export: where the boundary goes

**Status:** Accepted 2026-08-11. The owner took the recommendation on all three decisions.
**Issue:** [#11](https://github.com/Neixen-labs/trama/issues/11)
**Affects:** `docs/SPEC.md` section 8, `compiler/`, `solvers/epanet/`

| Decision | Answer |
|---|---|
| Where category 4 lives | An opaque section owned by a solver, which the core stores but has no code to parse |
| Where the `.inp` parser lives | `solvers/epanet/`, behind a generic importer interface the compiler discovers |
| The coordinate reference system | Required as `--source-crs`; refuse to compile without it |

The reasoning below is kept as written, including the options that were not taken — a decision without its rejected alternatives is impossible to revisit honestly.

## Why this needs deciding first

Two project rules point in opposite directions here.

`KICKOFF.md` phase 3 asks the compiler to accept EPANET `.inp` and requires a round trip: `.inp → .trama → .inp` with no functional loss.

`CLAUDE.md` states that no domain concept — pipe, pressure, pump — may exist outside `solvers/`, and calls it the rule most likely to be broken by a plausible-looking change.

An EPANET importer is exactly that plausible-looking change. Writing one without deciding where its knowledge lives puts a hydraulic model inside the domain-agnostic core, one reasonable commit at a time.

## What is actually in an `.inp` file

Four kinds of content, and they land in very different places.

**1. Topology.** `[JUNCTIONS]`, `[RESERVOIRS]`, `[TANKS]` are nodes; `[PIPES]`, `[PUMPS]`, `[VALVES]` are edges. This is `GRPH` with nothing left over. Entities are named by string IDs of up to 31 characters, and those names are how the rest of the file refers to them, so a name must survive import to make export possible at all.

**2. Geometry.** `[COORDINATES]` gives node positions, `[VERTICES]` the intermediate bends of a link. This is `GEOM`, with one problem: **an `.inp` declares no coordinate reference system.** The numbers may be UTM, a national grid, feet, or a CAD drawing with no georeference. TRAMA tiles in `EPSG:3857`, so something has to say what those numbers mean.

**3. Per-entity scalars.** Elevation, base demand, diameter, length, roughness, minor loss, initial status, tank levels. These fit `PROP` today, as opaque string keys. The core stores `diameter` without knowing what a diameter is, which is precisely the design.

Two catches. `PROP` has no unit field — a column is `f64`, not `f64 in millimetres` — and EPANET's units depend on `[OPTIONS] UNITS`: the same `300` is millimetres in an LPS file and inches in a GPM one. And several scalars are *references by name* to objects defined elsewhere in the file (a demand pattern, a pump head curve). The reference is a string and fits; the object it names does not.

**4. Everything with no entity to hang on.** `[PATTERNS]`, `[CURVES]`, `[CONTROLS]`, `[RULES]`, `[OPTIONS]`, `[TIMES]`, `[REPORT]`, `[ENERGY]`, `[REACTIONS]`, `[QUALITY]`, `[SOURCES]`, `[MIXING]`, `[EMITTERS]`, `[TAGS]`, `[LABELS]`, `[BACKDROP]`.

This is the category that forces a decision. A pattern is a time series of multipliers, a curve is a list of point pairs, and `[CONTROLS]` and `[RULES]` are a small imperative language with conditions and actions. None of it is a scalar attached to a node or an edge, so none of it fits `PROP`, and all of it is required for the file to simulate the same way after a round trip.

## Decision 1 — where category 4 lives

### Option A. Extend the core format to hold it

Add list-valued property types and a network-scoped property table to `PROP`, then encode patterns and curves natively.

Lossless and self-describing. But it grows the core format to serve one domain, and it does not actually reach: `[CONTROLS]` and `[RULES]` are a language, so even with list types they end up stored as strings. It spends a format change and still leaves the hard part unsolved.

### Option B. One opaque section, owned by a solver

Add a section kind that the core stores, checksums, and range-serves, but never parses. The record names its owner (`epanet`) and a media type; the payload is the leftovers, written verbatim by the importer and handed back untouched on export.

The core stays agnostic **by construction** rather than by discipline — it has no code that could interpret the bytes. A reader that does not know EPANET still renders the network, traverses the graph, and reads every scalar property; it simply cannot re-emit an `.inp`, which is the correct amount of ability for it to have.

The cost is honest: the file gains a region that is opaque to everything except one solver, and that region is a tempting place to put anything inconvenient. It needs a rule that says the file MUST remain fully usable when the section is dropped — no geometry, no topology, no property, and no channel declaration may live there.

### Option C. Keep the `.inp` as the source of truth

Import only what TRAMA can type. The EPANET solver receives the original `.inp` as an input asset, and the `.trama` is the map, not the model.

Cheapest and keeps the core untouched. It also gives up the round trip in `KICKOFF.md` and, more importantly, gives up the "one portable file" pillar for the only domain the project has committed to shipping first. Two files that must travel together are one file that does not exist.

### Recommendation: B

With four constraints written into the spec at the same time:

1. The section is opaque. The core MUST NOT parse it and MUST NOT branch on its contents.
2. It declares an owner id matching a `solver.toml` `id`, and a media type.
3. Dropping it MUST leave a valid, renderable, traversable file. It may only contain what would otherwise be lost, never what the format can already express.
4. A reader that does not recognise the owner MUST ignore the section, not reject the file — this is a minor-version addition under section 9.

## Decision 2 — where the `.inp` parser lives

Independent of decision 1, and the one that decides whether the non-negotiable rule survives contact with phase 5.

### Option A. In `compiler/`

`trama compile network.inp` works out of the box, and the phase 3 wording is satisfied literally. It also puts pumps, valves, and head-loss formulas inside the package that is supposed to know only nodes and edges.

### Option B. In `solvers/epanet/`, discovered by the compiler

The compiler declares a generic importer interface — given a source path, return graph, geometry, properties, and an optional opaque payload. `solvers/epanet/` implements it and registers under a `trama.importers` entry point group. The CLI enumerates what is installed; with the EPANET solver present, `trama compile network.inp` works, and the compiler still contains no hydraulics.

Cost: a plugin seam, and `uv run trama compile` on a bare checkout will not read `.inp` until the solver package is installed.

### Option C. A separate `trama-epanet` package

Same separation, distributed independently. Cleanest boundary, most release overhead, and it splits the EPANET work across two repositories or two packages when the solver and the importer share every line of parsing.

### Recommendation: B

The importer and the solver parse the same file and must agree exactly on it — keeping them in one package is what makes that agreement structural instead of a convention. It is also the only option under which `CLAUDE.md`'s rule stays literally true rather than approximately true, and the plugin seam is reusable: GeoPackage, CSV, and MVT importers land in the same slot.

## Decision 3 — the coordinate reference system

An `.inp` carries no CRS. Three ways to respond:

**A. Require the user to declare it**: `trama compile network.inp --source-crs EPSG:25830`. Refuse to compile without it. Explicit, and wrong input becomes a visible error instead of a network in the Atlantic.

**B. Guess from the coordinate ranges.** Convenient and occasionally right; when wrong, it is wrong silently, which is the same failure mode as #55 and now has a spec section explaining why we do not do that.

**C. Compile without georeference**, treating coordinates as a local plane. Honest for a CAD-drawn model, but it cannot tile, cannot sit on a basemap, and breaks pillar 1's range-by-tile loading.

**Recommendation: A**, with the original coordinates preserved verbatim in the decision-1 section so export restores exactly what was imported rather than a reprojected approximation of it. C stays available later as an explicit `--source-crs none` for models that genuinely have no georeference, once there is somewhere sensible to put them.

## What "no functional loss" should mean

Byte equality is not achievable and not worth chasing: comments, section order, and whitespace are not information about the network.

The proposed test: `.inp → .trama → .inp`, then run **both** files through the same EPANET binary and compare results. Equivalent means every node pressure and link flow agrees within solver tolerance at every reported timestep. Coverage on Net1 and Net3, as `KICKOFF.md` asks.

This definition has a useful property: it fails loudly if a pattern, a curve, or a control is dropped, and it stays quiet about reformatting, which is exactly the discrimination the round trip needs.

## What happens after the answers

1. A spec PR adds the section kind from decision 1 to `docs/SPEC.md` sections 2 and 8, and the round-trip definition above.
2. A compiler PR adds the importer interface from decision 2, with no EPANET in it.
3. `solvers/epanet/` gets the parser, the importer, the exporter, and the Net1/Net3 round-trip suite, then the OWA-EPANET WASM wrapper behind the existing solver contract.

Steps 2 and 3 are independent once step 1 lands.
