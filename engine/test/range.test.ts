// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { readFileSync } from "node:fs";
import { after, test } from "node:test";

import { decompress } from "fzstd";

import { fetchSection, httpRangeReader, openContainer } from "../src/range.js";
import { parseGeometry, parseGraph } from "../src/sections.js";

const fixture = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const inflate = (stored: Uint8Array) => decompress(stored);

type Served = { readonly url: string; readonly bytes: () => number; readonly close: () => Promise<void> };

/** Serves the fixture with real range support, counting what actually leaves the server. */
async function serve(options: { ignoreRange?: boolean } = {}): Promise<Served> {
  let sent = 0;
  const server: Server = createServer((request, response) => {
    const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? "");
    if (options.ignoreRange || range === null) {
      sent += fixture.byteLength;
      response.writeHead(200, { "Content-Length": String(fixture.byteLength) });
      response.end(fixture);
      return;
    }
    const body = fixture.subarray(Number(range[1]), Number(range[2]) + 1);
    sent += body.byteLength;
    response.writeHead(206, {
      "Content-Range": `bytes ${range[1]}-${range[2]}/${fixture.byteLength}`,
      "Content-Length": String(body.byteLength),
    });
    response.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("server has no port");
  return {
    url: `http://127.0.0.1:${address.port}/network.trama`,
    bytes: () => sent,
    close: () => new Promise((resolve) => server.close(() => resolve())),
  };
}

const servers: Served[] = [];
after(async () => {
  await Promise.all(servers.map((server) => server.close()));
});

async function started(options: { ignoreRange?: boolean } = {}): Promise<Served> {
  const server = await serve(options);
  servers.push(server);
  return server;
}

test("opens a container without downloading it", async () => {
  const server = await started();

  const container = await openContainer(httpRangeReader(server.url));

  assert.deepEqual(container.sections.map((section) => section.type), ["GEOM", "GEOM", "GRPH", "PROP", "STCH"]);
  // Header plus directory only: 64 + 5 x 64, and the header range is re-read to find the count.
  assert.equal(server.bytes(), 64 + (64 + 5 * 64));
  assert.ok(server.bytes() < fixture.byteLength);
});

test("fetches one tile without fetching its neighbour", async () => {
  const server = await started();
  const reader = httpRangeReader(server.url);
  const container = await openContainer(reader);
  const beforeTile = server.bytes();

  const tile = parseGeometry(await fetchSection(reader, container.sections[0]!, inflate));

  assert.equal(tile.paths.length, 1);
  const transferred = server.bytes() - beforeTile;
  assert.equal(transferred, Number(container.sections[0]!.storedBytes));
  assert.ok(server.bytes() < fixture.byteLength, `${server.bytes()} of ${fixture.byteLength} bytes`);
});

test("verifies a ranged section exactly as a local one", async () => {
  const server = await started();
  const reader = httpRangeReader(server.url);
  const container = await openContainer(reader);

  const graph = parseGraph(await fetchSection(reader, container.sections[2]!, inflate));

  assert.equal(graph.nodes.length, 4);
  assert.equal(graph.edges.length, 3);
});

test("rejects a server that ignores the Range header", async () => {
  const server = await started({ ignoreRange: true });

  await assert.rejects(openContainer(httpRangeReader(server.url)), /expected 206 for a range request, got 200/);
});

test("rejects a reader that returns the wrong number of bytes", async () => {
  const server = await started();
  const reader = httpRangeReader(server.url);
  const container = await openContainer(reader);
  const truncating = async (start: number, end: number) => (await reader(start, end)).subarray(0, 1);

  await assert.rejects(
    fetchSection(truncating, container.sections[2]!, inflate),
    /range reader returned the wrong length/,
  );
});

test("rejects a section whose bytes were corrupted in transit", async () => {
  const server = await started();
  const reader = httpRangeReader(server.url);
  const container = await openContainer(reader);
  const flipping = async (start: number, end: number) => {
    const bytes = await reader(start, end);
    bytes[bytes.length - 1] ^= 1;
    return bytes;
  };

  await assert.rejects(fetchSection(flipping, container.sections[3]!, inflate), /checksum mismatch|zstd|Invalid/i);
});
