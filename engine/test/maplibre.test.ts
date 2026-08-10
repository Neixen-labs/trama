// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decompress } from "fzstd";

import { parseContainer, type Section } from "../src/container.js";
import { createTramaLayer, tileMatrix, visibleTiles, type HostMap } from "../src/maplibre.js";

const bytes = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
const container = parseContainer(file);
const inflate = (stored: Uint8Array) => decompress(stored);

/** An independent column-major 4x4 multiply, to check tileMatrix's shortcut against. */
function multiply(a: readonly number[], b: readonly number[]): number[] {
  const result = new Array(16).fill(0);
  for (let column = 0; column < 4; column += 1) {
    for (let row = 0; row < 4; row += 1) {
      let sum = 0;
      for (let k = 0; k < 4; k += 1) sum += a[k * 4 + row]! * b[column * 4 + k]!;
      result[column * 4 + row] = sum;
    }
  }
  return result;
}

function placement(z: number, x: number, y: number): number[] {
  const scale = 1 / 2 ** z;
  // Column-major: scale on the diagonal, tile origin in the translation column.
  return [scale, 0, 0, 0, 0, scale, 0, 0, 0, 0, 1, 0, x * scale, y * scale, 0, 1];
}

test("composes the tile placement into the map matrix", () => {
  const mapMatrix = [2, 3, 0, 0, 5, 7, 0, 0, 0, 0, 1, 0, 11, 13, 0, 1];

  const composed = [...tileMatrix(mapMatrix, [14, 8024, 6177])];

  assert.deepEqual(
    composed.map((value) => Math.round(value * 1e6) / 1e6),
    multiply(mapMatrix, placement(14, 8024, 6177)).map((value) => Math.round(value * 1e6) / 1e6),
  );
});

test("maps a tile-local corner to the same clip position as its world coordinate", () => {
  const identity = [2, 0, 0, 0, 0, 2, 0, 0, 0, 0, 1, 0, -1, -1, 0, 1];
  const matrix = tileMatrix(identity, [14, 8024, 6177]);
  const scale = 1 / 2 ** 14;

  // The tile's top-left corner, (0,0) locally, is (8024, 6177) / 2^14 in world coordinates.
  const clipX = matrix[0]! * 0 + matrix[4]! * 0 + matrix[12]!;
  assert.ok(Math.abs(clipX - (2 * 8024 * scale - 1)) < 1e-6);
});

test("selects the tiles a viewport covers and no others", () => {
  const around = visibleTiles(container.sections, { west: -3.68, south: 40.41, east: -3.66, north: 40.42 });
  const elsewhere = visibleTiles(container.sections, { west: 2.1, south: 41.3, east: 2.2, north: 41.4 });

  assert.deepEqual(
    around.map((section) => section.key),
    [
      [14, 8024, 6177],
      [14, 8025, 6177],
    ],
  );
  assert.deepEqual(elsewhere, []);
});

test("ignores sections that are not geometry", () => {
  const world = visibleTiles(container.sections, { west: -180, south: -85, east: 180, north: 85 });

  assert.ok(world.every((section) => section.type === "GEOM"));
  assert.equal(world.length, 2);
});

function mockMap(bounds = { west: -3.68, south: 40.41, east: -3.66, north: 40.42 }) {
  let repaints = 0;
  const map: HostMap = {
    getBounds: () => ({
      getWest: () => bounds.west,
      getSouth: () => bounds.south,
      getEast: () => bounds.east,
      getNorth: () => bounds.north,
    }),
    triggerRepaint: () => {
      repaints += 1;
    },
  };
  return { map, repaints: () => repaints };
}

function mockGl() {
  const draws: unknown[] = [];
  const gl = new Proxy(
    {
      drawArraysInstanced: (...args: unknown[]) => draws.push(args),
      getAttribLocation: () => 0,
      getUniformLocation: () => "u",
      getShaderParameter: () => true,
      getProgramParameter: () => true,
      createShader: () => "shader",
      createProgram: () => "program",
    } as Record<string, unknown>,
    { get: (target, key) => target[key as string] ?? (() => 0) },
  );
  return { gl: gl as unknown as WebGL2RenderingContext, draws };
}

const layerOptions = {
  id: "trama",
  container,
  read: async (start: number, endInclusive: number) => new Uint8Array(file.slice(start, endInclusive + 1)),
  decompress: inflate,
  style: { widthPixels: 3, color: [1, 1, 1, 1] as const },
  resolutionPixels: () => [800, 600] as const,
};

test("fetches each visible tile once and repaints when it arrives", async () => {
  const { map, repaints } = mockMap();
  const { gl, draws } = mockGl();
  let requests = 0;
  const layer = createTramaLayer({
    ...layerOptions,
    read: async (start, end) => {
      requests += 1;
      return new Uint8Array(file.slice(start, end + 1));
    },
  });
  layer.onAdd(map, gl);

  layer.render(gl, new Float32Array(16));
  layer.render(gl, new Float32Array(16));
  assert.equal(draws.length, 0, "nothing draws until a tile has arrived");
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(requests, 2, "two visible tiles, fetched once each despite two renders");
  assert.equal(repaints(), 2);
  layer.render(gl, new Float32Array(16));
  assert.equal(draws.length, 2);
});

test("draws nothing when the viewport holds no tiles", async () => {
  const { map } = mockMap({ west: 2.1, south: 41.3, east: 2.2, north: 41.4 });
  const { gl, draws } = mockGl();
  const layer = createTramaLayer(layerOptions);
  layer.onAdd(map, gl);

  layer.render(gl, new Float32Array(16));
  await new Promise((resolve) => setImmediate(resolve));
  layer.render(gl, new Float32Array(16));

  assert.equal(draws.length, 0);
});

test("retries a tile whose fetch failed the next time it is visible", async () => {
  const { map } = mockMap();
  const { gl } = mockGl();
  let attempts = 0;
  const layer = createTramaLayer({
    ...layerOptions,
    read: async (start, end) => {
      attempts += 1;
      if (attempts <= 2) throw new Error("network down");
      return new Uint8Array(file.slice(start, end + 1));
    },
  });
  layer.onAdd(map, gl);

  layer.render(gl, new Float32Array(16));
  await new Promise((resolve) => setImmediate(resolve));
  layer.render(gl, new Float32Array(16));
  await new Promise((resolve) => setImmediate(resolve));

  assert.ok(attempts > 2, "a failed tile must not be cached as requested forever");
});

test("releases the renderer when removed", () => {
  const { map } = mockMap();
  const { gl, draws } = mockGl();
  const layer = createTramaLayer(layerOptions);
  layer.onAdd(map, gl);

  layer.onRemove(map, gl);
  layer.render(gl, new Float32Array(16));

  assert.equal(draws.length, 0);
});
