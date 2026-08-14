// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { GeometryTile } from "./sections.js";

/**
 * One instance per segment, ready for `bufferData`. A vertex shader expands each
 * instance into a quad along the segment normal; stroke width is a uniform, not part
 * of this buffer, because SPEC 3.3 makes width a camera-time and style-time decision.
 */
export type LineInstances = Readonly<{
  buffer: ArrayBuffer;
  count: number;
  strideBytes: number;
  /** Byte offsets of each attribute within one instance. */
  layout: Readonly<{ start: number; end: number; edgeIndex: number; nodes: number; along: number }>;
}>;

/**
 * An edge's source and target node indices in GRPH order, or `undefined` for an edge the
 * caller cannot place. They are what lets a node channel paint a line: the shader blends the
 * two node texels along the edge.
 */
export type EdgeEndpoints = (edgeIndex: number) => readonly [number, number] | undefined;

const STRIDE_BYTES = 24;
const LAYOUT = { start: 0, end: 4, edgeIndex: 8, nodes: 12, along: 20 } as const;
const ALONG_ONE = 65535;

/** Turns a parsed tile's paths into one instance per consecutive vertex pair. */
export function buildLineInstances(tile: GeometryTile, endpoints?: EdgeEndpoints): LineInstances {
  const count = tile.paths.reduce((total, path) => total + path.vertices.length / 2 - 1, 0);
  const buffer = new ArrayBuffer(count * STRIDE_BYTES);
  const view = new DataView(buffer);
  let at = 0;
  for (const path of tile.paths) {
    const [sourceNode, targetNode] = endpoints?.(path.edgeIndex) ?? [0, 0];
    // Arc-length fractions place each segment along its edge, so a source→target gradient
    // covers ground at the pace the line does rather than one step per vertex.
    // ponytail: fractions are per tile-local path, so an edge clipped by a tile border restarts
    // its gradient at the seam. Carry a path's starting fraction in GEOM if that ever shows.
    const along = fractions(path.vertices);
    for (let vertex = 0, segment = 0; vertex + 3 < path.vertices.length; vertex += 2, segment += 1) {
      view.setUint16(at + LAYOUT.start, path.vertices[vertex]!, true);
      view.setUint16(at + LAYOUT.start + 2, path.vertices[vertex + 1]!, true);
      view.setUint16(at + LAYOUT.end, path.vertices[vertex + 2]!, true);
      view.setUint16(at + LAYOUT.end + 2, path.vertices[vertex + 3]!, true);
      view.setUint32(at + LAYOUT.edgeIndex, path.edgeIndex, true);
      view.setUint32(at + LAYOUT.nodes, sourceNode, true);
      view.setUint32(at + LAYOUT.nodes + 4, targetNode, true);
      view.setUint16(at + LAYOUT.along, Math.round(along[segment]! * ALONG_ONE), true);
      view.setUint16(at + LAYOUT.along + 2, Math.round(along[segment + 1]! * ALONG_ONE), true);
      at += STRIDE_BYTES;
    }
  }
  return { buffer, count, strideBytes: STRIDE_BYTES, layout: LAYOUT };
}

/** Cumulative arc length at each vertex, normalized to [0, 1] over the whole path. */
function fractions(vertices: Uint16Array): readonly number[] {
  const cumulative = [0];
  for (let vertex = 0; vertex + 3 < vertices.length; vertex += 2) {
    const run = vertices[vertex + 2]! - vertices[vertex]!;
    const rise = vertices[vertex + 3]! - vertices[vertex + 1]!;
    cumulative.push(cumulative[cumulative.length - 1]! + Math.hypot(run, rise));
  }
  const total = cumulative[cumulative.length - 1]!;
  // A degenerate path has no length to distribute; every vertex sits at the source.
  return total > 0 ? cumulative.map((length) => length / total) : cumulative.map(() => 0);
}
