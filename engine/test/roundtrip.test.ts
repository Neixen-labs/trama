// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decompress } from "fzstd";

import { parseContainer } from "../src/container.js";
import { parseGeometry, parseGraph, readSection } from "../src/sections.js";

// Produced by the Python compiler from ../../fixtures/network.geojson; a compiler test
// asserts it still matches a fresh compile, so this cannot drift unnoticed.
const bytes = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
// Deliberately not handing fzstd a pre-sized output buffer: letting it derive the length
// keeps readSection's length check an independent verification rather than a tautology.
const inflate = (stored: Uint8Array) => decompress(stored);

test("reads every section of a compiler-produced container", () => {
  const container = parseContainer(file);

  assert.deepEqual(container.sections.map((section) => section.type), ["GEOM", "GEOM", "GRPH", "PROP", "STCH"]);
  assert.deepEqual(container.sections.slice(0, 2).map((section) => section.key), [
    [14, 8024, 6177],
    [14, 8025, 6177],
  ]);
  // Each call verifies the decoded length and the compiler's CRC-32C; a disagreement throws.
  for (const section of container.sections) readSection(file, section, inflate);
});

test("reads the graph the compiler wrote", () => {
  const container = parseContainer(file);
  const graph = parseGraph(readSection(file, container.sections[2]!, inflate));

  assert.deepEqual(graph.nodes.map((node) => node.id), [
    1374865284882970112n,
    8093211145144104199n,
    8261272653162081503n,
    12382409452541619382n,
  ]);
  assert.deepEqual(graph.edges.map((edge) => edge.id), [
    1565066226786393687n,
    2252215067368670575n,
    10119171071703032050n,
  ]);
  assert.deepEqual([...graph.csrOffsets], [0n, 1n, 2n, 4n, 6n]);
  assert.equal(graph.adjacency.length, 6);
  assert.deepEqual(graph.edges.map((edge) => edge.propertyRow), [0, 1, 2]);
});

test("reassembles an edge split across two tiles", () => {
  const container = parseContainer(file);
  const graph = parseGraph(readSection(file, container.sections[2]!, inflate));
  // The trunk is the only edge the compiler had to cut at a tile boundary.
  const trunk = graph.edges.find((edge) => edge.geometryRefCount > 1)!;
  const refs = graph.geometryRefs.slice(trunk.geometryRefStart, trunk.geometryRefStart + trunk.geometryRefCount);

  assert.deepEqual(refs.map((ref) => ref.geometryDirectoryIndex), [0, 1]);
  assert.ok(refs.every((ref) => ref.direction === 1));

  const [first, second] = refs.map((ref) => {
    const tile = parseGeometry(readSection(file, container.sections[ref.geometryDirectoryIndex]!, inflate));
    return tile.paths[ref.pathIndex]!;
  });
  assert.ok(first!.edgeIndex === second!.edgeIndex);
  // Quantization is per tile, so the shared vertex is the right edge of one tile and the left of the next.
  assert.equal(first!.vertices.at(-2), 65535);
  assert.equal(second!.vertices[0], 0);
  assert.equal(first!.vertices.at(-1), second!.vertices[1]);
});

test("carries no mesh for line geometry", () => {
  const container = parseContainer(file);

  for (const section of container.sections.filter((candidate) => candidate.type === "GEOM")) {
    const tile = parseGeometry(readSection(file, section, inflate));
    assert.equal(tile.meshVertexCount, 0);
    assert.equal(tile.meshIndexCount, 0);
  }
});
