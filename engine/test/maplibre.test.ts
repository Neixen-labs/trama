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

/** MapLibre's tile matrix, recorded so the test can assert what was asked for. */
function projectionInput(mainMatrix: number[]) {
  const asked: unknown[] = [];
  return {
    asked,
    input: {
      getProjectionData(params: { tileID: { canonical: { x: number; y: number; z: number } } }) {
        asked.push(params);
        return { mainMatrix };
      },
    },
  };
}

test("asks MapLibre for the matrix of the tile being drawn", () => {
  const { asked, input } = projectionInput(new Array(16).fill(0));

  tileMatrix(input, [14, 8024, 6177]);

  assert.deepEqual(asked, [{ tileID: { canonical: { x: 8024, y: 6177, z: 14 } } }]);
});

test("scales the tile matrix from EXTENT units to the renderer's [0,1] vertices", () => {
  // Column-major identity, so the scaling is visible per column.
  const identity = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
  const { input } = projectionInput(identity);

  const matrix = [...tileMatrix(input, [14, 8024, 6177])];

  // A vertex at local (1,1) must land where MapLibre puts tile coordinate (EXTENT, EXTENT).
  assert.equal(matrix[0], 8192);
  assert.equal(matrix[5], 8192);
  // The translation column is MapLibre's, untouched.
  assert.deepEqual(matrix.slice(12), [0, 0, 0, 1]);
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

  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  assert.equal(draws.length, 0, "nothing draws until a tile has arrived");
  await new Promise((resolve) => setImmediate(resolve));

  assert.equal(requests, 2, "two visible tiles, fetched once each despite two renders");
  assert.equal(repaints(), 2);
  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  assert.equal(draws.length, 2);
});

test("draws nothing when the viewport holds no tiles", async () => {
  const { map } = mockMap({ west: 2.1, south: 41.3, east: 2.2, north: 41.4 });
  const { gl, draws } = mockGl();
  const layer = createTramaLayer(layerOptions);
  layer.onAdd(map, gl);

  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  await new Promise((resolve) => setImmediate(resolve));
  layer.render(gl, projectionInput(new Array(16).fill(0)).input);

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

  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  await new Promise((resolve) => setImmediate(resolve));
  layer.render(gl, projectionInput(new Array(16).fill(0)).input);
  await new Promise((resolve) => setImmediate(resolve));

  assert.ok(attempts > 2, "a failed tile must not be cached as requested forever");
});

test("releases the renderer when removed", () => {
  const { map } = mockMap();
  const { gl, draws } = mockGl();
  const layer = createTramaLayer(layerOptions);
  layer.onAdd(map, gl);

  layer.onRemove(map, gl);
  layer.render(gl, projectionInput(new Array(16).fill(0)).input);

  assert.equal(draws.length, 0);
});
