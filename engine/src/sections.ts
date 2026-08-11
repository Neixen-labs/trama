// SPDX-License-Identifier: LicenseRef-BSL-1.1
import type { Section } from "./container.js";

/** Inflates one zstd frame. Supplied by the host so the parsers stay codec-agnostic. */
export type Decompress = (stored: Uint8Array, uncompressedBytes: number) => Uint8Array;

export type Node = Readonly<{ id: bigint; propertyRow: number }>;

export type Edge = Readonly<{
  id: bigint;
  sourceNodeIndex: number;
  targetNodeIndex: number;
  propertyRow: number;
  geometryRefStart: number;
  geometryRefCount: number;
  directed: boolean;
}>;

export type Adjacency = Readonly<{ edgeIndex: number; traversalDirection: number }>;

export type GeometryRef = Readonly<{ geometryDirectoryIndex: number; pathIndex: number; direction: number }>;

export type Graph = Readonly<{
  nodes: readonly Node[];
  edges: readonly Edge[];
  csrOffsets: BigUint64Array;
  adjacency: readonly Adjacency[];
  geometryRefs: readonly GeometryRef[];
}>;

export type GeometryPath = Readonly<{ edgeIndex: number; vertices: Uint16Array }>;

export type GeometryTile = Readonly<{ paths: readonly GeometryPath[]; meshVertexCount: number; meshIndexCount: number }>;

const NODE_BYTES = 8;
const EDGE_BYTES = 24;
const ADJACENCY_BYTES = 8;
const GEOMETRY_REF_BYTES = 12;
const PATH_BYTES = 16;

/** Unsigned LEB128 gaps between ascending ids (SPEC 4.1), as `count` `bigint`s. */
function identities(payload: Uint8Array, view: DataView, offset: number, count: number): BigUint64Array {
  const values = new BigUint64Array(count);
  let value = 0n;
  let at = offset;
  for (let index = 0; index < count; index += 1) {
    let gap = 0n;
    let shift = 0n;
    for (;;) {
      if (at >= payload.byteLength) throw new Error("identity block runs past the section");
      const group = view.getUint8(at);
      at += 1;
      gap |= BigInt(group & 0x7f) << shift;
      shift += 7n;
      if ((group & 0x80) === 0) break;
    }
    value += gap;
    values[index] = BigInt.asUintN(64, value);
  }
  return values;
}

/** Decompresses one section and rejects it unless its length and CRC-32C match the directory. */
export function readSection(file: ArrayBuffer, section: Section, decompress: Decompress): Uint8Array {
  if (section.codec !== 1) throw new Error("unsupported section codec");
  const uncompressedBytes = Number(section.uncompressedBytes);
  const stored = new Uint8Array(file, Number(section.offset), Number(section.storedBytes));
  const payload = decompress(stored, uncompressedBytes);
  if (payload.byteLength !== uncompressedBytes) throw new Error("section decoded length mismatch");
  if (crc32c(payload) !== section.crc32c) throw new Error("section checksum mismatch");
  return payload;
}

export function parseGraph(payload: Uint8Array): Graph {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const [nodeCount, edgeCount, adjacencyCount, geometryRefCount] = header(view, 4, 0);
  const [nodesOffset, edgesOffset, csrOffset, adjacencyOffset, refsOffset] = header(view, 5, 16);
  const [nodeIdsOffset, edgeIdsOffset] = header(view, 2, 36);
  bound(payload, nodesOffset, nodeCount, NODE_BYTES);
  bound(payload, edgesOffset, edgeCount, EDGE_BYTES);
  bound(payload, csrOffset, nodeCount + 1, 8);
  bound(payload, adjacencyOffset, adjacencyCount, ADJACENCY_BYTES);
  bound(payload, refsOffset, geometryRefCount, GEOMETRY_REF_BYTES);

  // SPEC 4.1: identity is a block of ascending gaps, decoded once in order. Reading the id of
  // one entity would mean decoding every id before it, and nothing here wants only one.
  const nodeIds = identities(payload, view, nodeIdsOffset, nodeCount);
  const edgeIds = identities(payload, view, edgeIdsOffset, edgeCount);

  const csrOffsets = new BigUint64Array(nodeCount + 1);
  for (let index = 0; index <= nodeCount; index += 1) csrOffsets[index] = view.getBigUint64(csrOffset + index * 8, true);
  if (csrOffsets[0] !== 0n || csrOffsets[nodeCount] !== BigInt(adjacencyCount)) {
    throw new Error("invalid CSR bounds");
  }
  for (let index = 1; index <= nodeCount; index += 1) {
    if (csrOffsets[index]! < csrOffsets[index - 1]!) throw new Error("invalid CSR bounds");
  }

  return {
    csrOffsets,
    nodes: build(nodeCount, (index) => {
      const at = nodesOffset + index * NODE_BYTES;
      return { id: nodeIds[index]!, propertyRow: view.getUint32(at, true) };
    }),
    edges: build(edgeCount, (index) => {
      const at = edgesOffset + index * EDGE_BYTES;
      const edge = {
        id: edgeIds[index]!,
        sourceNodeIndex: view.getUint32(at, true),
        targetNodeIndex: view.getUint32(at + 4, true),
        propertyRow: view.getUint32(at + 8, true),
        geometryRefStart: view.getUint32(at + 12, true),
        geometryRefCount: view.getUint32(at + 16, true),
        directed: (view.getUint32(at + 20, true) & 1) !== 0,
      };
      if (edge.sourceNodeIndex >= nodeCount || edge.targetNodeIndex >= nodeCount) {
        throw new Error("edge references a missing node");
      }
      if (edge.geometryRefStart + edge.geometryRefCount > geometryRefCount) {
        throw new Error("edge references missing geometry refs");
      }
      return edge;
    }),
    adjacency: build(adjacencyCount, (index) => {
      const at = adjacencyOffset + index * ADJACENCY_BYTES;
      const edgeIndex = view.getUint32(at, true);
      if (edgeIndex >= edgeCount) throw new Error("adjacency references a missing edge");
      return { edgeIndex, traversalDirection: view.getInt8(at + 4) };
    }),
    geometryRefs: build(geometryRefCount, (index) => {
      const at = refsOffset + index * GEOMETRY_REF_BYTES;
      return {
        geometryDirectoryIndex: view.getUint32(at, true),
        pathIndex: view.getUint32(at + 4, true),
        direction: view.getInt8(at + 8),
      };
    }),
  };
}

export function parseGeometry(payload: Uint8Array): GeometryTile {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  const [pathCount, pathVertexCount, meshVertexCount, meshIndexCount] = header(view, 4, 0);
  const [pathsOffset, verticesOffset] = header(view, 2, 16);
  bound(payload, pathsOffset, pathCount, PATH_BYTES);
  bound(payload, verticesOffset, pathVertexCount, 4);

  return {
    meshVertexCount,
    meshIndexCount,
    paths: build(pathCount, (index) => {
      const at = pathsOffset + index * PATH_BYTES;
      const firstVertex = view.getUint32(at + 4, true);
      const vertexCount = view.getUint32(at + 8, true);
      if (vertexCount < 2 || firstVertex + vertexCount > pathVertexCount) throw new Error("invalid path vertex range");
      return {
        edgeIndex: view.getUint32(at, true),
        // ponytail: a copy, not a view — the payload is byte-aligned but not 2-byte aligned in general.
        vertices: new Uint16Array(payload.slice(verticesOffset + firstVertex * 4, verticesOffset + (firstVertex + vertexCount) * 4).buffer),
      };
    }),
  };
}

function header(view: DataView, count: number, offset: number): number[] {
  if (offset + count * 4 > view.byteLength) throw new Error("section is shorter than its header");
  return Array.from({ length: count }, (_, index) => view.getUint32(offset + index * 4, true));
}

function bound(payload: Uint8Array, offset: number, count: number, stride: number): void {
  if (offset + count * stride > payload.byteLength) throw new Error("section field exceeds payload bounds");
}

function build<T>(count: number, at: (index: number) => T): readonly T[] {
  return Array.from({ length: count }, (_, index) => at(index));
}

const CRC_TABLE = Uint32Array.from({ length: 256 }, (_, byte) => {
  let crc = byte;
  for (let bit = 0; bit < 8; bit += 1) crc = crc & 1 ? (crc >>> 1) ^ 0x82f63b78 : crc >>> 1;
  return crc >>> 0;
});

export function crc32c(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = (crc >>> 8) ^ CRC_TABLE[(crc ^ byte) & 0xff]!;
  return (crc ^ 0xffffffff) >>> 0;
}
