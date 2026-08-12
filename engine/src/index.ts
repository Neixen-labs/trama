// SPDX-License-Identifier: LicenseRef-BSL-1.1
/**
 * `@trama/core`: reading a TRAMA container in a browser, and drawing it with time on it.
 *
 * A library, not a framework. Nothing here reaches for a global, mounts itself, or decides how
 * bytes arrive: a reader is a function from a byte range to bytes, decompression is handed in,
 * and the MapLibre adapter is one module you may ignore. That is why the published package has
 * no runtime dependencies at all — the pieces it works with are described by types rather than
 * imported.
 *
 * Subpaths are exported too, so `@trama/core/maplibre` costs nothing to anyone not on MapLibre.
 */

// The container: header, section directory, and the sections themselves.
export { HEADER_RANGE, directoryRange, parseContainer, parsePrefix, type Container, type Section } from "./container.js";
export {
  crc32c,
  parseGeometry,
  parseGraph,
  readSection,
  type Adjacency,
  type Decompress,
  type Edge,
  type GeometryPath,
  type GeometryRef,
  type GeometryTile,
  type Graph,
  type Node,
} from "./sections.js";

// Getting the bytes: over HTTP ranges, or from anywhere else that can answer one.
export { httpRangeReader, type RangeReader } from "./range.js";
export { cachedInOpfs, forget, type CacheOptions, type OpfsStorage } from "./opfs.js";

// State over time: the declared channels, the ring buffer, and the texture a shader reads.
export { StateRing, parseStateChannels, type StateChannel, type StateRingOptions } from "./state.js";
export { createStateTexture, type StateTexture } from "./state-texture.js";
export { CONTRACT_VERSION, SolverFailed, solveDeltas, type SolveError, type SolveRequest } from "./solver.js";

// Drawing: instanced lines, and the MapLibre custom layer that mounts them.
export { buildLineInstances, type LineInstances } from "./lines.js";
export { createLineRenderer, type LineRenderer, type LineStyle, type StateStyle } from "./line-renderer.js";
export {
  createTramaLayer,
  tileMatrix,
  visibleTiles,
  type Bounds,
  type CustomLayer,
  type HostMap,
  type LayerState,
  type LayerStyle,
  type RenderInput,
  type TramaLayerOptions,
} from "./maplibre.js";

// Touring a network: a walk, a route through it, and where to point a camera along one.
export {
  buildRoute,
  edgePolyline,
  routeFrom,
  sampleRoute,
  startNode,
  toLonLat,
  unquantize,
  walk,
  zoomFor,
  type LonLat,
  type Point,
  type Route,
  type Step,
  type TileGeometry,
  type View,
} from "./flythrough.js";
