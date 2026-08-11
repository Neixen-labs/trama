// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { GeometryTile, Graph } from "./sections.js";

/**
 * Web Mercator metres — the space the format already stores, so a route needs no projection
 * to measure itself. Distances are therefore Mercator metres, not ground metres: at latitude
 * 40 they run about 30% long. That scales how fast the camera appears to move and nothing
 * else, so the route corrects for it nowhere.
 */
export type Point = readonly [number, number];

export type LonLat = readonly [number, number];

/** A parsed `GEOM` record with its tile key, addressed the way `GeometryRef` addresses it. */
export type TileGeometry = Readonly<{ key: readonly [number, number, number]; tile: GeometryTile }>;

/** Where the camera sits and which way it looks. */
export type View = Readonly<{ center: LonLat; bearing: number }>;

/** One traversal step: an edge and the direction the walk crosses it in. */
export type Step = Readonly<{ edgeIndex: number; direction: number }>;

/**
 * A polyline with its cumulative lengths, so sampling is a binary search rather than a walk
 * from the start on every frame.
 */
export type Route = Readonly<{ points: readonly Point[]; cumulative: Float64Array; length: number }>;

const WORLD = 40075016.68557849;
const EXTENT = 65535;

/** The inverse of the SPEC 3.1 quantization, which is what makes stored geometry traversable. */
export function unquantize(qx: number, qy: number, key: readonly [number, number, number]): Point {
  const [z, x, y] = key;
  const width = WORLD / 2 ** z;
  return [-WORLD / 2 + x * width + (qx / EXTENT) * width, WORLD / 2 - y * width - (qy / EXTENT) * width];
}

export function toLonLat([x, y]: Point): LonLat {
  return [(x / WORLD) * 360, (Math.atan(Math.sinh((y / WORLD) * 2 * Math.PI)) * 180) / Math.PI];
}

/**
 * One edge's centerline in traversal order, following its geometry references across the tile
 * boundaries an edge may span. Returns nothing when a referenced tile has not been fetched:
 * a partial centerline would put the camera through a wall.
 */
export function edgePolyline(
  graph: Graph,
  step: Step,
  tiles: ReadonlyMap<number, TileGeometry>,
): readonly Point[] {
  const edge = graph.edges[step.edgeIndex];
  if (edge === undefined) return [];
  const points: Point[] = [];
  for (let at = 0; at < edge.geometryRefCount; at += 1) {
    const reference = graph.geometryRefs[edge.geometryRefStart + at]!;
    const found = tiles.get(reference.geometryDirectoryIndex);
    const path = found?.tile.paths[reference.pathIndex];
    if (found === undefined || path === undefined) return [];
    const piece: Point[] = [];
    for (let vertex = 0; vertex + 1 < path.vertices.length; vertex += 2) {
      piece.push(unquantize(path.vertices[vertex]!, path.vertices[vertex + 1]!, found.key));
    }
    if (reference.direction < 0) piece.reverse();
    // Consecutive references meet at a shared vertex; keeping both would be a zero-length segment.
    for (const point of piece) if (!same(points[points.length - 1], point)) points.push(point);
  }
  if (step.direction < 0) points.reverse();
  return points;
}

/**
 * A connected tour of everything reachable from a node: take the first unvisited edge, and when
 * a node has none left, retrace the edge you arrived by and try again.
 *
 * Retracing is what makes it a tour rather than a dead end. A walk that merely refuses to repeat
 * an edge stops at the first exhausted node — on Net3 that is five edges out of a hundred and
 * nineteen — and a camera cannot teleport to the rest. Backtracking crosses some edges twice and
 * visits them all, which is the trade a fly-through wants.
 *
 * ponytail: depth-first, so the order is whatever the CSR happens to list first, and the tour is
 * up to twice the network's length. A shorter one is a route-inspection problem, and nothing here
 * needs the optimum.
 */
export function walk(graph: Graph, startNodeIndex: number, maxEdges: number): readonly Step[] {
  const visited = new Set<number>();
  const steps: Step[] = [];
  const retreat: Step[] = [];
  let node = startNodeIndex;
  while (steps.length < maxEdges) {
    const first = Number(graph.csrOffsets[node] ?? 0n);
    const last = Number(graph.csrOffsets[node + 1] ?? 0n);
    let chosen;
    for (let at = first; at < last; at += 1) {
      const entry = graph.adjacency[at]!;
      if (!visited.has(entry.edgeIndex)) {
        chosen = entry;
        break;
      }
    }
    if (chosen === undefined) {
      const back = retreat.pop();
      if (back === undefined) break;
      steps.push(back);
      node = endOf(graph, back);
      continue;
    }
    visited.add(chosen.edgeIndex);
    const step = { edgeIndex: chosen.edgeIndex, direction: chosen.traversalDirection };
    steps.push(step);
    retreat.push({ edgeIndex: step.edgeIndex, direction: -step.direction });
    node = endOf(graph, step);
  }
  return steps;
}

/** The node a step arrives at. */
function endOf(graph: Graph, step: Step): number {
  const edge = graph.edges[step.edgeIndex]!;
  return step.direction > 0 ? edge.targetNodeIndex : edge.sourceNodeIndex;
}

/** The node to start from: an endpoint if the network has one, so the walk covers more of it. */
export function startNode(graph: Graph): number {
  for (let node = 0; node + 1 < graph.csrOffsets.length; node += 1) {
    if (graph.csrOffsets[node + 1]! - graph.csrOffsets[node]! === 1n) return node;
  }
  return 0;
}

/** Measures a polyline once so sampling it later is a binary search. */
export function routeFrom(points: readonly Point[]): Route {
  const cumulative = new Float64Array(points.length);
  for (let at = 1; at < points.length; at += 1) {
    cumulative[at] = cumulative[at - 1]! + distance(points[at - 1]!, points[at]!);
  }
  return { points, cumulative, length: cumulative[points.length - 1] ?? 0 };
}

/** Chains a walk's centerlines into one measurable polyline. */
export function buildRoute(graph: Graph, steps: readonly Step[], tiles: ReadonlyMap<number, TileGeometry>): Route {
  const points: Point[] = [];
  for (const step of steps) {
    for (const point of edgePolyline(graph, step, tiles)) {
      if (!same(points[points.length - 1], point)) points.push(point);
    }
  }
  return routeFrom(points);
}

/**
 * The camera at `metres` along the route, aiming at a point `lookAhead` further on.
 *
 * Aiming ahead rather than at the next vertex is what keeps the flight smooth: the heading
 * turns into a corner before reaching it, the way a driver does, so a polyline needs no spline
 * fitted to it. `lookAhead` is the whole tuning knob — larger cuts corners, smaller tracks the
 * centerline and jerks at sharp ones.
 */
export function sampleRoute(route: Route, metres: number, lookAhead: number): View {
  const at = positionAt(route, metres);
  const ahead = positionAt(route, Math.min(metres + lookAhead, route.length));
  return { center: toLonLat(at), bearing: bearingOf(at, ahead) };
}

/**
 * A zoom that frames the flight for the network it is over, since a fixed one is wrong by orders
 * of magnitude across real data: an EPANET network whose `.inp` carries arbitrary units can span
 * metres, a municipal one tens of kilometres.
 *
 * It shows `1/FRAMED` of the route across the viewport, which keeps the sense of speed roughly
 * constant. Tile size is MapLibre's 512, and the result is clamped to the zooms a map will accept.
 */
export function zoomFor(route: Route, viewportPixels: number): number {
  const span = route.length / FRAMED;
  if (!(span > 0) || !(viewportPixels > 0)) return 16;
  const zoom = Math.log2((WORLD * viewportPixels) / (512 * span));
  return Math.max(1, Math.min(22, zoom));
}

const FRAMED = 25;

function positionAt(route: Route, metres: number): Point {
  const { points, cumulative } = route;
  if (points.length === 0) return [0, 0];
  if (points.length === 1 || metres <= 0) return points[0]!;
  if (metres >= route.length) return points[points.length - 1]!;
  let low = 0;
  let high = points.length - 1;
  while (low + 1 < high) {
    const middle = (low + high) >> 1;
    if (cumulative[middle]! <= metres) low = middle;
    else high = middle;
  }
  const span = cumulative[low + 1]! - cumulative[low]!;
  const fraction = span === 0 ? 0 : (metres - cumulative[low]!) / span;
  const from = points[low]!;
  const to = points[low + 1]!;
  return [from[0] + (to[0] - from[0]) * fraction, from[1] + (to[1] - from[1]) * fraction];
}

/** Compass degrees, which is what a map camera's bearing wants. Mercator y grows northward. */
function bearingOf(from: Point, to: Point): number {
  const bearing = (Math.atan2(to[0] - from[0], to[1] - from[1]) * 180) / Math.PI;
  return (bearing + 360) % 360;
}

function distance(from: Point, to: Point): number {
  return Math.hypot(to[0] - from[0], to[1] - from[1]);
}

function same(from: Point | undefined, to: Point): boolean {
  return from !== undefined && from[0] === to[0] && from[1] === to[1];
}
