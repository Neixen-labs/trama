// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { Container, Section } from "./container.js";
import { createLineRenderer, type LineRenderer, type LineStyle } from "./line-renderer.js";
import { buildLineInstances, type LineInstances } from "./lines.js";
import { fetchSection, type RangeReader } from "./range.js";
import { parseGeometry, type Decompress } from "./sections.js";

/**
 * MapLibre's custom-layer surface, declared structurally so `@trama/core` needs no
 * `maplibre-gl` dependency. Any object shaped like this can host the layer.
 */
export type CustomLayer = Readonly<{
  id: string;
  type: "custom";
  renderingMode: "2d";
  onAdd(map: HostMap, gl: WebGL2RenderingContext): void;
  render(gl: WebGL2RenderingContext, input: RenderInput): void;
  onRemove(map: HostMap, gl: WebGL2RenderingContext): void;
}>;

/**
 * What MapLibre 5 and 6 hand `render`. Only `getProjectionData` is used: it returns the matrix
 * for a specific tile, which is the supported way to place tile-local geometry and the only one
 * that also holds under globe projection.
 */
export type RenderInput = Readonly<{
  getProjectionData(params: { tileID: { canonical: { x: number; y: number; z: number } } }): {
    mainMatrix: Float32Array | number[];
  };
}>;

/** Tile coordinate space MapLibre's tile matrices expect. Measured against a real map, not assumed. */
const EXTENT = 8192;

export type HostMap = Readonly<{
  getBounds(): { getWest(): number; getSouth(): number; getEast(): number; getNorth(): number };
  triggerRepaint(): void;
}>;

export type LayerStyle = Omit<LineStyle, "matrix" | "resolutionPixels">;

export type TramaLayerOptions = Readonly<{
  id: string;
  container: Container;
  read: RangeReader;
  decompress: Decompress;
  style: LayerStyle;
  /** Drawing-buffer size in pixels; the width uniform is in pixels, so the layer must be told. */
  resolutionPixels: () => readonly [number, number];
}>;

/**
 * The matrix placing one tile's geometry on screen.
 *
 * MapLibre's tile matrix expects coordinates in `[0, EXTENT]`; the renderer's vertices are
 * tile-normalized `[0,1]`, so the first two columns are scaled by EXTENT.
 */
export function tileMatrix(input: RenderInput, key: readonly [number, number, number]): Float32Array {
  const [z, x, y] = key;
  const { mainMatrix } = input.getProjectionData({ tileID: { canonical: { x, y, z } } });
  const result = new Float32Array(16);
  for (let row = 0; row < 4; row += 1) {
    result[row] = mainMatrix[row]! * EXTENT;
    result[4 + row] = mainMatrix[4 + row]! * EXTENT;
    result[8 + row] = mainMatrix[8 + row]!;
    result[12 + row] = mainMatrix[12 + row]!;
  }
  return result;
}

export type Bounds = Readonly<{ west: number; south: number; east: number; north: number }>;

/**
 * The GEOM records inside a viewport, so panning fetches only what came into view.
 *
 * ponytail: a viewport crossing the antimeridian (west > east) selects nothing. Split it into
 * two ranges when a dataset actually spans it.
 */
export function visibleTiles(sections: readonly Section[], bounds: Bounds): readonly Section[] {
  return sections.filter((section) => {
    if (section.type !== "GEOM") return false;
    const [z, x, y] = section.key;
    const [westX, northY] = tileOf(bounds.west, bounds.north, z);
    const [eastX, southY] = tileOf(bounds.east, bounds.south, z);
    return x >= westX && x <= eastX && y >= northY && y <= southY;
  });
}

function tileOf(longitude: number, latitude: number, z: number): readonly [number, number] {
  const tiles = 2 ** z;
  const clamped = Math.max(Math.min(latitude, 85.05112878), -85.05112878);
  const radians = (clamped * Math.PI) / 180;
  const x = ((longitude + 180) / 360) * tiles;
  const y = ((1 - Math.log(Math.tan(radians) + 1 / Math.cos(radians)) / Math.PI) / 2) * tiles;
  return [clamp(Math.floor(x), tiles), clamp(Math.floor(y), tiles)];
}

function clamp(value: number, tiles: number): number {
  return Math.max(0, Math.min(tiles - 1, value));
}

export function createTramaLayer(options: TramaLayerOptions): CustomLayer {
  const loaded = new Map<Section, LineInstances>();
  const requested = new Set<Section>();
  let renderer: LineRenderer | null = null;
  let host: HostMap | null = null;

  const request = (section: Section) => {
    if (requested.has(section)) return;
    requested.add(section);
    fetchSection(options.read, section, options.decompress)
      .then((payload) => {
        loaded.set(section, buildLineInstances(parseGeometry(payload)));
        host?.triggerRepaint();
      })
      .catch(() => {
        // A failed tile is retried the next time it comes into view rather than poisoning the cache.
        requested.delete(section);
      });
  };

  return {
    id: options.id,
    type: "custom",
    renderingMode: "2d",
    onAdd(map, gl) {
      host = map;
      renderer = createLineRenderer(gl);
    },
    render(_gl, input) {
      if (renderer === null || host === null) return;
      const bounds = host.getBounds();
      const visible = visibleTiles(options.container.sections, {
        west: bounds.getWest(),
        south: bounds.getSouth(),
        east: bounds.getEast(),
        north: bounds.getNorth(),
      });
      for (const section of visible) {
        const instances = loaded.get(section);
        if (instances === undefined) {
          request(section);
          continue;
        }
        renderer.draw(instances, {
          ...options.style,
          matrix: tileMatrix(input, section.key),
          resolutionPixels: options.resolutionPixels(),
        });
      }
    },
    onRemove() {
      renderer?.dispose();
      renderer = null;
      host = null;
      loaded.clear();
      requested.clear();
    },
  };
}
