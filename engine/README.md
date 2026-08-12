# @trama/core

Read a [TRAMA](https://trama.build) container in the browser and render network state over time on the GPU.

A library, not a framework, and **with no runtime dependencies**. Nothing here reaches for a global, mounts itself, or decides how bytes arrive: a reader is a function from a byte range to bytes, decompression is handed in, and the MapLibre adapter is one module you may ignore.

```bash
npm install @trama/core
```

## What it does

A `.trama` file is one binary container: a header, a section directory with offsets, pre-tessellated geometry, a CSR graph with stable IDs, typed properties, and the state channels solvers may write. This package reads it over HTTP range requests — header, then directory, then only the tiles in view — and draws it as instanced lines whose colour and width come from a GPU texture indexed by entity ID, so scrubbing a day of state is a texture row lookup rather than a re-upload.

```ts
import { parseContainer, httpRangeReader, cachedInOpfs, createTramaLayer, StateRing } from "@trama/core";
import { decompress } from "fzstd";

const read = cachedInOpfs(httpRangeReader("https://example.invalid/city.trama"), { key: "city.trama" });
const container = parseContainer(await read(0, 4095));

map.addLayer(
  createTramaLayer({
    id: "network",
    container,
    read,
    decompress: (stored) => decompress(stored),
    style: { widthPixels: 2.5, color: [0.49, 0.82, 1, 1] },
    resolutionPixels: () => [map.getCanvas().width, map.getCanvas().height],
  }),
);
```

Zstd is not bundled: `decompress` is a parameter, so you choose the implementation and pay for it only if your containers are compressed. MapLibre is not imported either — the adapter describes the host map it needs with a type, so `@trama/core/maplibre` costs nothing to anyone not using it.

## Subpaths

Every module is exported individually, so a bundler can drop what you do not touch.

| | |
|---|---|
| `@trama/core/container` | header, section directory, CRC-32C checks |
| `@trama/core/sections` | geometry, CSR graph, properties |
| `@trama/core/range` | HTTP range reader |
| `@trama/core/opfs` | cache ranges in the origin private file system, so a container read once needs no network |
| `@trama/core/state` | declared channels and the temporal ring buffer |
| `@trama/core/state-texture` | the R32F texture a shader samples |
| `@trama/core/line-renderer` | instanced WebGL2 lines with screen-constant width |
| `@trama/core/maplibre` | custom layer, mounted on a map you already have |
| `@trama/core/flythrough` | routes, camera sampling, and a zoom that frames one |
| `@trama/core/solver` | the solver contract's SSE client |

## Status

Pre-alpha. WebGL2 today; WebGPU is not implemented. The renderer draws lines — points and polygons are not there yet. The format is versioned by `docs/SPEC.md` in the repository and a container states the version it was written to.

## Licence

Business Source License 1.1 — source-available, not OSI open source. See `LICENSE`.
