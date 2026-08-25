// SPDX-License-Identifier: BUSL-1.1
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

  // Node IDs come from the SPEC 4.1 grid cell, so they change with the fixture, never with a run.
  assert.deepEqual(graph.nodes.map((node) => node.id), [
    2195025027559335999n,
    13309651855582348747n,
    14204704510718787469n,
    15756743663225437053n,
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

/**
 * The package's own index is the API. `openContainer` and `fetchSection` were missing from it —
 * the two functions the whole range-loading idea is made of — and the demo in this repository
 * imported them from `dist/range.js` instead, which is how the omission survived: the one
 * consumer close enough to notice was also close enough to reach around it.
 */
test("the index exports the range-loading entry point, not just the reader that feeds it", async () => {
  const api = await import("../src/index.js");
  for (const name of ["openContainer", "fetchSection", "httpRangeReader"]) {
    assert.equal(typeof (api as Record<string, unknown>)[name], "function", `${name} is part of the API`);
  }

  // And it works through that door: header and directory over ranges, then one verified section.
  const read = async (start: number, endInclusive: number) => new Uint8Array(file.slice(start, endInclusive + 1));
  const container = await api.openContainer(read);
  const graph = api.parseGraph(await api.fetchSection(read, section(container, "GRPH"), inflate));
  assert.ok(graph.nodes.length > 0);
});

function section(container: { sections: readonly { type: string }[] }, type: string) {
  const found = container.sections.find((candidate) => candidate.type === type);
  if (found === undefined) throw new Error(`the fixture has no ${type}`);
  return found as never;
}
