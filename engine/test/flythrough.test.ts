// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decompress } from "fzstd";

import { parseContainer } from "../src/container.js";
import {
  buildRoute,
  edgePolyline,
  routeFrom,
  sampleRoute,
  startNode,
  toLonLat,
  unquantize,
  walk,
  zoomFor,
  type Point,
  type Step,
  type TileGeometry,
} from "../src/flythrough.js";
import { parseGeometry, parseGraph, readSection, type Graph } from "../src/sections.js";

const bytes = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
const inflate = (stored: Uint8Array) => decompress(stored);

/** The fixture's graph and its GEOM records, keyed the way a `GeometryRef` addresses them. */
function fixture(): { graph: Graph; tiles: Map<number, TileGeometry> } {
  const container = parseContainer(file);
  const tiles = new Map<number, TileGeometry>();
  let graph: Graph | undefined;
  container.sections.forEach((section, index) => {
    if (section.type === "GEOM") {
      tiles.set(index, { key: section.key, tile: parseGeometry(readSection(file, section, inflate)) });
    } else if (section.type === "GRPH") {
      graph = parseGraph(readSection(file, section, inflate));
    }
  });
  assert.ok(graph !== undefined, "the fixture has a graph");
  return { graph, tiles };
}

/** A square of four nodes, each edge one straight path, so traversal order is checkable by eye. */
function square(): { graph: Graph; tiles: Map<number, TileGeometry> } {
  const edges = [
    { source: 0, target: 1 },
    { source: 1, target: 2 },
    { source: 2, target: 3 },
  ];
  const adjacency: { edgeIndex: number; traversalDirection: number }[] = [];
  const csrOffsets = new BigUint64Array(5);
  for (let node = 0; node < 4; node += 1) {
    edges.forEach((edge, edgeIndex) => {
      if (edge.source === node) adjacency.push({ edgeIndex, traversalDirection: 1 });
      if (edge.target === node) adjacency.push({ edgeIndex, traversalDirection: -1 });
    });
    csrOffsets[node + 1] = BigInt(adjacency.length);
  }
  const graph: Graph = {
    nodes: [0, 1, 2, 3].map((index) => ({ id: BigInt(index), propertyRow: index })),
    edges: edges.map((edge, index) => ({
      id: BigInt(index),
      sourceNodeIndex: edge.source,
      targetNodeIndex: edge.target,
      propertyRow: index,
      geometryRefStart: index,
      geometryRefCount: 1,
      directed: false,
    })),
    csrOffsets,
    adjacency,
    geometryRefs: [0, 1, 2].map((pathIndex) => ({ geometryDirectoryIndex: 9, pathIndex, direction: 1 })),
  };
  const tiles = new Map<number, TileGeometry>([
    [
      9,
      {
        key: [0, 0, 0],
        tile: {
          meshVertexCount: 0,
          meshIndexCount: 0,
          paths: [
            { edgeIndex: 0, vertices: Uint16Array.from([0, 0, 100, 0]) },
            { edgeIndex: 1, vertices: Uint16Array.from([100, 0, 100, 100]) },
            { edgeIndex: 2, vertices: Uint16Array.from([100, 100, 0, 100]) },
          ],
        },
      },
    ],
  ]);
  return { graph, tiles };
}

test("unquantize inverts the SPEC 3.1 grid", () => {
  const [x, y] = unquantize(32768, 32768, [0, 0, 0]);

  // The middle of the world tile is the origin of Web Mercator, give or take half a step.
  assert.ok(Math.abs(x) < 400, `x ${x}`);
  assert.ok(Math.abs(y) < 400, `y ${y}`);
});

test("toLonLat puts the Mercator origin on null island and keeps the poles finite", () => {
  assert.deepEqual(toLonLat([0, 0]), [0, 0]);

  const [longitude, latitude] = toLonLat(unquantize(65535, 0, [0, 0, 0]));
  assert.ok(Math.abs(longitude - 180) < 0.01, `longitude ${longitude}`);
  assert.ok(Math.abs(latitude - 85.051) < 0.01, `latitude ${latitude}`);
});

test("walk follows the graph outward and then retraces its way home", () => {
  const { graph } = square();

  const steps = walk(graph, 0, 100);

  assert.deepEqual(
    steps.map((step) => [step.edgeIndex, step.direction]),
    [
      [0, 1],
      [1, 1],
      [2, 1],
      [2, -1],
      [1, -1],
      [0, -1],
    ],
  );
});

test("walk crosses an edge backwards when it arrives at the target", () => {
  const { graph } = square();

  const steps = walk(graph, 3, 3);

  assert.deepEqual(
    steps.map((step) => [step.edgeIndex, step.direction]),
    [
      [2, -1],
      [1, -1],
      [0, -1],
    ],
  );
});

test("walk terminates once it has retraced back to where it started", () => {
  const { graph } = square();

  // Three edges out and three back, never the infinite loop a naive retreat would give.
  assert.equal(walk(graph, 0, 1000).length, 6);
});

test("walk honours its edge budget", () => {
  const { graph } = square();

  assert.equal(walk(graph, 0, 2).length, 2);
});

test("startNode prefers an endpoint so the walk covers more of the network", () => {
  const { graph } = square();

  // Nodes 0 and 3 each have one edge; the first one found wins.
  assert.equal(startNode(graph), 0);
});

test("edgePolyline reverses a centerline crossed against its stored order", () => {
  const { graph, tiles } = square();
  const forward = edgePolyline(graph, { edgeIndex: 0, direction: 1 }, tiles);

  const backward = edgePolyline(graph, { edgeIndex: 0, direction: -1 }, tiles);

  assert.deepEqual(backward, [...forward].reverse());
});

test("edgePolyline yields nothing when the tile holding it is not loaded", () => {
  const { graph } = square();

  assert.deepEqual(edgePolyline(graph, { edgeIndex: 0, direction: 1 }, new Map()), []);
});

test("buildRoute chains edges without repeating the vertex they share", () => {
  const { graph, tiles } = square();

  const route = buildRoute(graph, walk(graph, 0, 3), tiles);

  // Four corners of the square, not six endpoints.
  assert.equal(route.points.length, 4);
  assert.ok(route.length > 0);
});

test("buildRoute keeps the return leg, which doubles back over the same corners", () => {
  const { graph, tiles } = square();

  const route = buildRoute(graph, walk(graph, 0, 100), tiles);

  // Out over four corners and back over three: the turn itself is not repeated.
  assert.equal(route.points.length, 7);
  assert.deepEqual(route.points[0], route.points[6]);
});

test("zoomFor frames a short network closer than a long one", () => {
  const short = routeFrom([
    [0, 0],
    [0, 40],
  ]);
  const long = routeFrom([
    [0, 0],
    [0, 40000],
  ]);

  assert.ok(zoomFor(short, 1280) > zoomFor(long, 1280), "a 40 m network needs more zoom than a 40 km one");
  assert.ok(zoomFor(long, 1280) > 10 && zoomFor(long, 1280) < 18, `zoom ${zoomFor(long, 1280)}`);
});

test("zoomFor stays inside the range a map accepts", () => {
  const microscopic = routeFrom([
    [0, 0],
    [0, 0.001],
  ]);
  const planetary = routeFrom([
    [0, 0],
    [0, 20000000],
  ]);

  assert.equal(zoomFor(microscopic, 1280), 22);
  assert.ok(zoomFor(planetary, 1280) >= 1);
  assert.equal(zoomFor(routeFrom([]), 1280), 16, "a route with no length falls back rather than returning Infinity");
});

test("sampleRoute interpolates along a segment rather than snapping to vertices", () => {
  const route = routeFrom([
    [0, 0],
    [0, 1000],
  ]);

  const view = sampleRoute(route, 500, 1);

  assert.equal(view.center[0], 0);
  assert.ok(view.center[1] > 0, "halfway north of the equator");
  assert.equal(Math.round(view.bearing), 0, "due north");
});

test("sampleRoute reads compass bearings off the Mercator axes", () => {
  const east = routeFrom([
    [0, 0],
    [1000, 0],
  ]);
  const south = routeFrom([
    [0, 0],
    [0, -1000],
  ]);

  assert.equal(Math.round(sampleRoute(east, 0, 100).bearing), 90);
  assert.equal(Math.round(sampleRoute(south, 0, 100).bearing), 180);
});

test("sampleRoute aims into a corner before reaching it", () => {
  const corner = routeFrom([
    [0, 0],
    [0, 1000],
    [1000, 1000],
  ]);

  // Standing 100 m short of the corner: tracking the segment would still read due north.
  const tight = sampleRoute(corner, 900, 10);
  const wide = sampleRoute(corner, 900, 400);

  assert.equal(Math.round(tight.bearing), 0);
  assert.ok(wide.bearing > 20 && wide.bearing < 90, `bearing ${wide.bearing}`);
});

test("sampleRoute clamps past either end instead of running off the polyline", () => {
  const route = routeFrom([
    [0, 0],
    [0, 1000],
  ]);

  assert.deepEqual(sampleRoute(route, -50, 10).center, sampleRoute(route, 0, 10).center);
  assert.deepEqual(sampleRoute(route, 1e9, 10).center, toLonLat([0, 1000]));
});

test("sampleRoute survives a route with no geometry", () => {
  const empty = routeFrom([]);

  assert.equal(empty.length, 0);
  assert.deepEqual(sampleRoute(empty, 10, 10).center, [0, 0]);
});

test("flies a continuous route over the real fixture", () => {
  const { graph, tiles } = fixture();

  const steps = walk(graph, startNode(graph), 500);
  const route = buildRoute(graph, steps, tiles);

  assert.ok(steps.length > 0, "the fixture graph is walkable");
  assert.ok(route.length > 0, "the route has length");
  // Continuity is the property the camera depends on: no jump between consecutive vertices
  // larger than the longest single edge could plausibly be.
  const longest = route.points.reduce((worst, point, index) => {
    if (index === 0) return worst;
    const previous = route.points[index - 1] as Point;
    return Math.max(worst, Math.hypot(point[0] - previous[0], point[1] - previous[1]));
  }, 0);
  assert.ok(longest < route.length, `longest hop ${longest} of ${route.length}`);

  const start = sampleRoute(route, 0, 50);
  assert.ok(Number.isFinite(start.center[0]) && Number.isFinite(start.center[1]));
  assert.ok(start.bearing >= 0 && start.bearing < 360);
});

/** Every step of the walk must actually reach the node the next step starts from. */
test("the walk's steps join end to end on the real fixture", () => {
  const { graph } = fixture();
  const steps: readonly Step[] = walk(graph, startNode(graph), 200);

  let node = startNode(graph);
  for (const step of steps) {
    const edge = graph.edges[step.edgeIndex]!;
    const from = step.direction > 0 ? edge.sourceNodeIndex : edge.targetNodeIndex;
    assert.equal(from, node, `step on edge ${step.edgeIndex} starts elsewhere`);
    node = step.direction > 0 ? edge.targetNodeIndex : edge.sourceNodeIndex;
  }
});
