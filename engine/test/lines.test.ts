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
    nodes: [view.getUint32(instances.layout.nodes, true), view.getUint32(instances.layout.nodes + 4, true)],
    along: [view.getUint16(instances.layout.along, true), view.getUint16(instances.layout.along + 2, true)],
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
  const placed = instanceAt(instances, 0);

  assert.deepEqual(placed.start, [first.vertices[0], first.vertices[1]]);
  assert.deepEqual(placed.end, [first.vertices[2], first.vertices[3]]);
  assert.equal(placed.edgeIndex, first.edgeIndex);
});

test("splits a multi-segment path into one instance per pair", () => {
  const instances = buildLineInstances({
    meshVertexCount: 0,
    meshIndexCount: 0,
    paths: [{ edgeIndex: 7, vertices: Uint16Array.from([0, 0, 10, 10, 20, 30]) }],
  });

  assert.equal(instances.count, 2);
  const [first, second] = [instanceAt(instances, 0), instanceAt(instances, 1)];
  assert.deepEqual([first.start, first.end, first.edgeIndex], [[0, 0], [10, 10], 7]);
  assert.deepEqual([second.start, second.end, second.edgeIndex], [[10, 10], [20, 30], 7]);
});

test("carries the edge's nodes and each segment's arc fraction along it", () => {
  // Two segments of 30 and 10 units: the middle vertex sits at 3/4 of the arc, not at 1/2.
  const instances = buildLineInstances(
    {
      meshVertexCount: 0,
      meshIndexCount: 0,
      paths: [{ edgeIndex: 7, vertices: Uint16Array.from([0, 0, 0, 30, 0, 40]) }],
    },
    (edgeIndex) => (edgeIndex === 7 ? [4, 9] : undefined),
  );

  const [first, second] = [instanceAt(instances, 0), instanceAt(instances, 1)];
  assert.deepEqual(first.nodes, [4, 9]);
  assert.deepEqual(second.nodes, [4, 9]);
  assert.deepEqual(first.along, [0, Math.round(0.75 * 65535)]);
  assert.deepEqual(second.along, [Math.round(0.75 * 65535), 65535]);
});

test("an edge the lookup cannot place falls back to node zero", () => {
  const instances = buildLineInstances(
    {
      meshVertexCount: 0,
      meshIndexCount: 0,
      paths: [{ edgeIndex: 7, vertices: Uint16Array.from([0, 0, 10, 0]) }],
    },
    () => undefined,
  );

  assert.deepEqual(instanceAt(instances, 0).nodes, [0, 0]);
});

test("produces an empty buffer for a tile with no paths", () => {
  const instances = buildLineInstances({ meshVertexCount: 0, meshIndexCount: 0, paths: [] });

  assert.equal(instances.count, 0);
  assert.equal(instances.buffer.byteLength, 0);
});
