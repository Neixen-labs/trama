// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decompress } from "fzstd";

import { parseContainer } from "../src/container.js";
import { buildLineInstances } from "../src/lines.js";
import { parseGeometry, readSection, type GeometryTile } from "../src/sections.js";

const bytes = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
const inflate = (stored: Uint8Array) => decompress(stored);

function fixtureTile(directoryIndex: number): GeometryTile {
  const container = parseContainer(file);
  return parseGeometry(readSection(file, container.sections[directoryIndex]!, inflate));
}

function instanceAt(instances: ReturnType<typeof buildLineInstances>, index: number) {
  const view = new DataView(instances.buffer, index * instances.strideBytes, instances.strideBytes);
  return {
    start: [view.getUint16(instances.layout.start, true), view.getUint16(instances.layout.start + 2, true)],
    end: [view.getUint16(instances.layout.end, true), view.getUint16(instances.layout.end + 2, true)],
    edgeIndex: view.getUint32(instances.layout.edgeIndex, true),
  };
}

test("emits one instance per segment of a real tile", () => {
  const tile = fixtureTile(1);
  const instances = buildLineInstances(tile);

  const segments = tile.paths.reduce((total, path) => total + path.vertices.length / 2 - 1, 0);
  assert.equal(instances.count, segments);
  assert.equal(instances.buffer.byteLength, segments * instances.strideBytes);
});

test("carries each segment's endpoints and edge index", () => {
  const tile = fixtureTile(1);
  const instances = buildLineInstances(tile);
  const first = tile.paths[0]!;

  assert.deepEqual(instanceAt(instances, 0), {
    start: [first.vertices[0], first.vertices[1]],
    end: [first.vertices[2], first.vertices[3]],
    edgeIndex: first.edgeIndex,
  });
});

test("splits a multi-segment path into one instance per pair", () => {
  const instances = buildLineInstances({
    meshVertexCount: 0,
    meshIndexCount: 0,
    paths: [{ edgeIndex: 7, vertices: Uint16Array.from([0, 0, 10, 10, 20, 30]) }],
  });

  assert.equal(instances.count, 2);
  assert.deepEqual(instanceAt(instances, 0), { start: [0, 0], end: [10, 10], edgeIndex: 7 });
  assert.deepEqual(instanceAt(instances, 1), { start: [10, 10], end: [20, 30], edgeIndex: 7 });
});

test("produces an empty buffer for a tile with no paths", () => {
  const instances = buildLineInstances({ meshVertexCount: 0, meshIndexCount: 0, paths: [] });

  assert.equal(instances.count, 0);
  assert.equal(instances.buffer.byteLength, 0);
});
