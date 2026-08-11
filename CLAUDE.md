# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Current state

Phase 3 of `KICKOFF.md`, with phase 4 just started. `solvers/epanet/` is still an empty placeholder; do not invent build commands for it.

- `core/` — Rust workspace, run with cargo from that directory: `cargo test --release`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. `trama-format` is the container, `trama-cli` the command line, `trama-epanet` every hydraulic concept in the project, `trama-solver` the contract runtime, `trama-example` a reference solver, `trama-wasm` the browser entry point. `mesh_index_count` is still `0` (no tessellation yet) and only `LineString` and `Point` features are accepted.
- `engine/` — TypeScript package `@trama/core`, run with npm from that directory: `npm test`, `npm run check`. It reads the container header and section directory only.
- `site/` — static landing page, no build step.

`KICKOFF.md` is the authoritative roadmap and constraint list. Read it before any non-trivial change. Tasks marked `[HUMANO]` are the owner's, not yours; every `[DECISIÓN]` must be presented with 2-3 options and a recommendation before implementing.

`docs/WORKBOARD.md` is the coordination contract for parallel work: claim the GitHub issue by comment, branch from `origin/main` in a separate worktree, one issue one branch one PR, merge only on green CI.

## Architecture

TRAMA is a network-map engine built on three pillars that constrain every design choice:

1. **Open binary format** — one file, little-endian, header + section directory with offsets so a client fetches only what it needs over HTTP range requests. v0 sections: `GEOMETRY` (pre-tessellated tile buffers, ready for direct `bufferData`), `GRAPH` (stable u64 IDs, CSR adjacency, edge→geometry vertex-order reference), `PROPS` (typed key-value with a global key dictionary), `STATE_CHANNELS` (declaration only — the file never contains state, only the contract solvers write against).
2. **GPU rendering with time** — runtime state lives as a GPU texture indexed by entity ID, with a temporal ring buffer for video-style scrubbing and in-shader interpolation. WebGPU with WebGL2 fallback, mounted as a custom layer on MapLibre (no basemap of its own).
3. **Solver plugins** — `solver.toml` manifest, WASM/WASI sandbox and/or HTTP server, both emitting the same packed state delta `(entity_id: u64, channel: u16, t: f32, value: f32)`. That delta is what feeds the engine's GPU texture, so the client cannot tell local from remote.

The pieces connect as: Python compiler (`compiler/`, PyPI `trama-engine`) produces `.trama` files → TypeScript runtime (`engine/`, npm `@trama/core`) range-loads and renders them → solvers (`solvers/`) read the graph and write state channels back into the ring buffer.

## Non-negotiable rules

- **Domain-agnostic core.** No domain concept (pipe, pressure, road, voltage) may exist outside `core/trama-epanet` and any future domain crate. The core knows nodes, edges, typed properties, and state channels. This is the rule most likely to be violated by a plausible-looking change.
- **The spec leads the code.** If code needs something `docs/SPEC.md` does not cover, change the spec first in a separate PR. Never improvise format.
- **BSL 1.1 header in every source file.** The core is source-available, not OSI open source; the repo's own workflow files carry `SPDX-License-Identifier: Apache-2.0` since they are not core.
- **English in code and technical docs**, Spanish allowed in discussion. Conventional commits, small PRs.
- No premature optimization outside a measurable "criterio de hecho" from `KICKOFF.md`.
- After each phase: update `README.md` with real status and append a short ADR to `docs/DECISIONS.md`.

## Planned stack (owner constraints — do not substitute)

- Rust for the producing side: the format, the CLI, the solvers, and the browser module. Owner decision on 2026-08-11, replacing the Python stack that came before; `docs/DECISIONS.md` records why and what it cost. Dependencies stay few: `serde_json`, `sha2`, `zstd`, `proj4rs`, `crs-definitions`, `epanet-sys`, `clap`. CI: `cargo fmt`, `cargo clippy -D warnings`, `cargo test`.
- TypeScript engine as a pure library with framework adapters, not a framework.
- Backend (when one exists): FastAPI + uvicorn, SQLAlchemy 2.0 async, Pydantic v2. Frontend: Next.js App Router + Tailwind + shadcn/ui + TanStack Query.
- GitHub Actions for open source, Azure DevOps for private work.

## Site

`site/` deploys to Cloudflare Pages on every push to `main` touching `site/**` (`.github/workflows/deploy-pages.yml`, needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`). Open `site/index.html` directly in a browser to preview. `FORM_ENDPOINT` at the top of its inline script is intentionally empty until the waitlist provider is chosen; the form degrades to a message when unset. Landing copy is Spanish; keep `prefers-reduced-motion` handling intact when editing the canvas animation.

## Contributions

External PRs require the contributor to post the ICLA statement from `CONTRIBUTOR_LICENSE_AGREEMENT.md` as a PR comment before merge. A PR is not accepted until its license status is clear.
