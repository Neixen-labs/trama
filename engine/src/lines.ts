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
  layout: Readonly<{ start: number; end: number; edgeIndex: number }>;
}>;

const STRIDE_BYTES = 12;
const LAYOUT = { start: 0, end: 4, edgeIndex: 8 } as const;

/** Turns a parsed tile's paths into one instance per consecutive vertex pair. */
export function buildLineInstances(tile: GeometryTile): LineInstances {
  const count = tile.paths.reduce((total, path) => total + path.vertices.length / 2 - 1, 0);
  const buffer = new ArrayBuffer(count * STRIDE_BYTES);
  const view = new DataView(buffer);
  let at = 0;
  for (const path of tile.paths) {
    for (let vertex = 0; vertex + 3 < path.vertices.length; vertex += 2) {
      view.setUint16(at + LAYOUT.start, path.vertices[vertex]!, true);
      view.setUint16(at + LAYOUT.start + 2, path.vertices[vertex + 1]!, true);
      view.setUint16(at + LAYOUT.end, path.vertices[vertex + 2]!, true);
      view.setUint16(at + LAYOUT.end + 2, path.vertices[vertex + 3]!, true);
      view.setUint32(at + LAYOUT.edgeIndex, path.edgeIndex, true);
      at += STRIDE_BYTES;
    }
  }
  return { buffer, count, strideBytes: STRIDE_BYTES, layout: LAYOUT };
}
