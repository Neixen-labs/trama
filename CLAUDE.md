# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

Phases 1-3 of `KICKOFF.md` are done and phases 4 and 5 are most of the way: a network compiles, range-loads, renders with a time scrub, and is solved by EPANET or a router. There is no `solvers/` directory and no `compiler/` one — the solvers are crates in `core/` and the compiler is `trama-cli`. `README.md` holds the honest per-piece table; keep it current rather than duplicating it here.

Known gaps, so they are not rediscovered as bugs: no WebGPU — deferred by decision, because MapLibre 6 hands a custom layer a WebGL2 context and only that one — no polygons, also deferred by decision to v1, nothing published to npm or crates.io. `docs/DECISIONS.md` says why for each.

- `core/` — Rust workspace, run with cargo from that directory: `cargo test --release`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. `trama-format` is the container, `trama-cli` the command line, the GeoPackage writer and the `grid` benchmark generator, `trama-solver` the contract runtime, `trama-example` a reference solver over HTTP+SSE, `trama-wasm` the browser entry point. Domain crates: `trama-epanet` (hydraulics, `.inp` in and out, EPANET 2.3 native and over WASI), `trama-swmm` (drainage networks, `.inp` in and out, no engine yet — reached by `--importer swmm` since EPANET owns the suffix), `trama-roads` (OpenStreetMap import), `trama-routing` (fastest paths). `trama-swmm` depends on `trama-epanet` without default features for `inp` and `Reprojection` only — the C engine stays behind the `solver` feature gate. `trama-trace` is domain-free by design — reach, isolation, critical edges — despite reading like water.
- Only `LineString` and `Point` features are accepted, and a `Point` annotates a node an edge endpoint already made rather than creating one — it is rejected when it matches none, joined on the SPEC 4.2 cell. `trama compile --points` reads a CSV through that same rule. `export --to mvt` writes a tile pyramid; the protobuf is hand-encoded in `trama-cli/src/mvt.rs` with no dependency. Mesh vertex and index counts are zero because SPEC §3.3 forbids tessellating lines, not for want of a tessellator; a mesh is what a polygon would need.
- `engine/` — TypeScript package `@trama/core`, run with npm from that directory: `npm test`, `npm run check`, `npm run demo` (needs a solver running), `npm run bench -- --container <file>`. It range-reads every section with CRC checks, renders instanced lines as a MapLibre custom layer, feeds an R32F state texture from a ring buffer, consumes solver deltas over SSE, flies the graph, and caches reads in OPFS. `src/` imports nothing at runtime — keep it that way.
- `site/` — landing page plus the playground under `site/demo/`, which is a build: `site/demo/build.sh` gathers the engine, the WASM compiler and the examples into ignored `vendor/` and `examples/`, and generates `sw.js` from `sw.template.js`.
- CI: `core-checks.yml` and `engine-checks.yml` gate pull requests. `bench.yml` does not — a hosted runner has no GPU — so it only runs on `main` and its red goes unnoticed unless someone looks.

`KICKOFF.md` is the authoritative roadmap and constraint list. Read it before any non-trivial change. Tasks marked `[HUMANO]` are the owner's, not yours; every `[DECISIÓN]` must be presented with 2-3 options and a recommendation before implementing.

`docs/WORKBOARD.md` is the coordination contract for parallel work: claim the GitHub issue by comment, branch from `origin/main` in a separate worktree, one issue one branch one PR, merge only on green CI.

## Architecture

TRAMA is a network-map engine built on three pillars that constrain every design choice:

1. **Open binary format** — one file, little-endian, header + section directory with offsets so a client fetches only what it needs over HTTP range requests. v0 sections: `GEOMETRY` (pre-tessellated tile buffers, ready for direct `bufferData`), `GRAPH` (stable u64 IDs, CSR adjacency, edge→geometry vertex-order reference), `PROPS` (typed key-value with a global key dictionary), `STATE_CHANNELS` (declaration only — the file never contains state, only the contract solvers write against).
2. **GPU rendering with time** — runtime state lives as a GPU texture indexed by entity ID, with a temporal ring buffer for video-style scrubbing and in-shader interpolation. WebGPU with WebGL2 fallback, mounted as a custom layer on MapLibre (no basemap of its own).
3. **Solver plugins** — `solver.toml` manifest, WASM/WASI sandbox and/or HTTP server, both emitting the same packed state delta `(entity_id: u64, channel: u16, t: f32, value: f32)`. That delta is what feeds the engine's GPU texture, so the client cannot tell local from remote.

The pieces connect as: the Rust compiler (`core/trama-cli`, and the same code as WASM in `core/trama-wasm`) produces `.trama` files → the TypeScript runtime (`engine/`, npm `@trama/core`) range-loads and renders them → solvers (crates in `core/`, over HTTP+SSE or WASI) read the graph and write state channels back into the ring buffer.

## Non-negotiable rules

- **Domain-agnostic core.** No domain concept (pipe, pressure, road, voltage) may exist outside the domain crates — today `trama-epanet`, `trama-roads`, `trama-routing` — and any future one. The core knows nodes, edges, typed properties, and state channels. This is the rule most likely to be violated by a plausible-looking change.
- **The spec leads the code.** If code needs something `docs/SPEC.md` does not cover, change the spec first in a separate PR. Never improvise format.
- **BSL 1.1 header in every source file.** The core is source-available, not OSI open source; the repo's own workflow files carry `SPDX-License-Identifier: Apache-2.0` since they are not core.
- **English in code and technical docs**, Spanish allowed in discussion. Conventional commits, small PRs.
- No premature optimization outside a measurable "criterio de hecho" from `KICKOFF.md`.
- After each phase: update `README.md` with real status and append a short ADR to `docs/DECISIONS.md`.

## Planned stack (owner constraints — do not substitute)

- Rust for the producing side: the format, the CLI, the solvers, and the browser module. Owner decision on 2026-08-11, replacing the Python stack that came before; `docs/DECISIONS.md` records why and what it cost. Dependencies stay few: `serde_json`, `sha2`, `zstd`, `proj4rs`, `crs-definitions`, `epanet-sys`, `clap`, and `rusqlite` in `trama-cli` alone — it carries SQLite for the GeoPackage writer and must never reach `trama-format` or `trama-wasm`. CI: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`.
- TypeScript engine as a pure library with framework adapters, not a framework.
- Backend (when one exists): FastAPI + uvicorn, SQLAlchemy 2.0 async, Pydantic v2. Frontend: Next.js App Router + Tailwind + shadcn/ui + TanStack Query.
- GitHub Actions for open source, Azure DevOps for private work.

## Site

`site/` deploys to Cloudflare Pages on every push to `main` touching `site/**`, `engine/src/**` or `core/**` (`.github/workflows/deploy-pages.yml`, needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`). The workflow runs `site/demo/build.sh`, so the playground on production is built from that push. Open `site/index.html` directly in a browser to preview the landing page; the playground needs a server, because it fetches modules. The waitlist posts to Formspree through `FORM_ENDPOINT`. Landing copy is Spanish; keep `prefers-reduced-motion` handling intact when editing the canvas animation.

## Contributions

External PRs require the contributor to post the ICLA statement from `CONTRIBUTOR_LICENSE_AGREEMENT.md` as a PR comment before merge. A PR is not accepted until its license status is clear.
