// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import test from "node:test";

import type { Section } from "../src/container.js";
import { crc32c, parseGeometry, parseGraph, readSection } from "../src/sections.js";

function graphPayload(): Uint8Array {
  const payload = new Uint8Array(152);
  const view = new DataView(payload.buffer);
  [2, 1, 2, 1, 36, 68, 100, 124, 140].forEach((value, index) => view.setUint32(index * 4, value, true));
  view.setBigUint64(36, 10n, true);
  view.setBigUint64(52, 20n, true);
  view.setUint32(60, 1, true);
  view.setBigUint64(68, 99n, true);
  [0, 1, 0, 0, 1, 0].forEach((value, index) => view.setUint32(76 + index * 4, value, true));
  [0n, 1n, 2n].forEach((value, index) => view.setBigUint64(100 + index * 8, value, true));
  view.setUint32(124, 0, true);
  view.setInt8(128, 1);
  view.setUint32(132, 0, true);
  view.setInt8(136, -1);
  view.setUint32(140, 0, true);
  view.setUint32(144, 0, true);
  view.setInt8(148, 1);
  return payload;
}

function geometryPayload(): Uint8Array {
  const payload = new Uint8Array(60);
  const view = new DataView(payload.buffer);
  [1, 3, 0, 0, 32, 48, 60, 60].forEach((value, index) => view.setUint32(index * 4, value, true));
  [0, 0, 3, 0].forEach((value, index) => view.setUint32(32 + index * 4, value, true));
  [1, 2, 3, 4, 5, 6].forEach((value, index) => view.setUint16(48 + index * 2, value, true));
  return payload;
}

function sectionFor(payload: Uint8Array, overrides: Partial<Section> = {}): { file: ArrayBuffer; section: Section } {
  const file = new ArrayBuffer(64 + payload.byteLength);
  new Uint8Array(file, 64).set(payload);
  return {
    file,
    section: {
      type: "GRPH",
      required: true,
      key: [0, 0, 0],
      offset: 64n,
      storedBytes: BigInt(payload.byteLength),
      uncompressedBytes: BigInt(payload.byteLength),
      crc32c: crc32c(payload),
      codec: 1,
      ...overrides,
    },
  };
}

const identity = (stored: Uint8Array) => stored;

test("crc32c matches the Castagnoli check vector", () => {
  assert.equal(crc32c(new TextEncoder().encode("123456789")), 0xe3069283);
});

test("reads a section whose length and checksum match the directory", () => {
  const { file, section } = sectionFor(graphPayload());

  assert.deepEqual(readSection(file, section, identity), graphPayload());
});

test("rejects a section whose checksum disagrees", () => {
  const { file, section } = sectionFor(graphPayload(), { crc32c: 1 });

  assert.throws(() => readSection(file, section, identity), /section checksum mismatch/);
});

test("rejects a section whose decoded length disagrees", () => {
  const { file, section } = sectionFor(graphPayload(), { uncompressedBytes: 151n });

  assert.throws(() => readSection(file, section, identity), /section decoded length mismatch/);
});

test("rejects a codec other than zstd", () => {
  const { file, section } = sectionFor(graphPayload(), { codec: 0 });

  assert.throws(() => readSection(file, section, identity), /unsupported section codec/);
});

test("parses nodes, edges, CSR and geometry refs", () => {
  const graph = parseGraph(graphPayload());

  assert.deepEqual(graph.nodes.map((node) => node.id), [10n, 20n]);
  assert.equal(graph.nodes[1]?.propertyRow, 1);
  assert.deepEqual(graph.edges[0], {
    id: 99n,
    sourceNodeIndex: 0,
    targetNodeIndex: 1,
    propertyRow: 0,
    geometryRefStart: 0,
    geometryRefCount: 1,
    directed: false,
  });
  assert.deepEqual([...graph.csrOffsets], [0n, 1n, 2n]);
  assert.deepEqual(graph.adjacency.map((entry) => entry.traversalDirection), [1, -1]);
  assert.deepEqual(graph.geometryRefs[0], { geometryDirectoryIndex: 0, pathIndex: 0, direction: 1 });
});

test("rejects a CSR whose last offset disagrees with the adjacency count", () => {
  const payload = graphPayload();
  new DataView(payload.buffer).setBigUint64(116, 3n, true);

  assert.throws(() => parseGraph(payload), /invalid CSR bounds/);
});

test("rejects an edge pointing at a missing node", () => {
  const payload = graphPayload();
  new DataView(payload.buffer).setUint32(80, 7, true);

  assert.throws(() => parseGraph(payload), /edge references a missing node/);
});

test("parses paths and their vertex runs", () => {
  const tile = parseGeometry(geometryPayload());

  assert.equal(tile.paths.length, 1);
  assert.equal(tile.paths[0]?.edgeIndex, 0);
  assert.deepEqual([...tile.paths[0]!.vertices], [1, 2, 3, 4, 5, 6]);
  assert.equal(tile.meshIndexCount, 0);
});

test("rejects a path running past the vertex array", () => {
  const payload = geometryPayload();
  new DataView(payload.buffer).setUint32(40, 4, true);

  assert.throws(() => parseGeometry(payload), /invalid path vertex range/);
});
