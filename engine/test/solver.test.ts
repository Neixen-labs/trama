// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { createServer, type Server } from "node:http";
import { after, test } from "node:test";

import { CONTRACT_VERSION, SolverFailed, solveDeltas } from "../src/solver.js";

const servers: Server[] = [];
after(() => servers.forEach((server) => server.close()));

/** Emits a scripted event stream, and records the request the client sent. */
async function serve(script: (write: (event: string, data: string) => void) => void) {
  const requests: unknown[] = [];
  const server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on("data", (chunk: Buffer) => chunks.push(chunk));
    request.on("end", () => {
      requests.push(JSON.parse(Buffer.concat(chunks).toString() || "{}"));
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      script((event, data) => response.write(`event: ${event}\ndata: ${data}\n\n`));
      response.end();
    });
  });
  servers.push(server);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("server has no port");
  return { url: `http://127.0.0.1:${address.port}/solve`, requests };
}

function delta(entityId: bigint, channelId: number, t: number, value: number): Uint8Array {
  const record = new Uint8Array(18);
  const view = new DataView(record.buffer);
  view.setBigUint64(0, entityId, true);
  view.setUint16(8, channelId, true);
  view.setFloat32(10, t, true);
  view.setFloat32(14, value, true);
  return record;
}

const base64 = (bytes: Uint8Array) => Buffer.from(bytes).toString("base64");

async function collect(url: string, overrides: Record<string, unknown> = {}) {
  const chunks: Uint8Array[] = [];
  for await (const payload of solveDeltas(url, {
    tramaUrl: "http://127.0.0.1/network.trama",
    t0Seconds: 0,
    t1Seconds: 60,
    ...overrides,
  })) {
    chunks.push(payload);
  }
  return chunks;
}

test("sends the request shape the contract specifies", async () => {
  const { url, requests } = await serve((write) => {
    write("ready", '{"solver_id":"x"}');
    write("complete", '{"delta_count":0}');
  });

  await collect(url, { params: { channel: "flow" } });

  assert.deepEqual(requests[0], {
    contract_version: CONTRACT_VERSION,
    trama: { url: "http://127.0.0.1/network.trama" },
    params: { channel: "flow" },
    t0_seconds: 0,
    t1_seconds: 60,
  });
});

test("yields each delta event's decoded payload", async () => {
  const first = delta(10n, 7, 0, 1.5);
  const second = delta(20n, 7, 0, 2.5);
  const { url } = await serve((write) => {
    write("ready", "{}");
    write("delta", base64(first));
    write("delta", base64(second));
    write("complete", '{"delta_count":2}');
  });

  const chunks = await collect(url);

  assert.deepEqual(chunks, [first, second]);
});

test("accepts several deltas batched into one event", async () => {
  const batch = new Uint8Array(36);
  batch.set(delta(10n, 7, 0, 1), 0);
  batch.set(delta(20n, 7, 0, 2), 18);
  const { url } = await serve((write) => {
    write("ready", "{}");
    write("delta", base64(batch));
    write("complete", '{"delta_count":2}');
  });

  const chunks = await collect(url);

  assert.equal(chunks.length, 1);
  assert.equal(chunks[0]?.byteLength, 36);
});

test("reassembles an event split across chunk boundaries", async () => {
  const payload = base64(delta(10n, 7, 0, 1.5));
  const server = createServer((request, response) => {
    request.resume();
    request.on("end", async () => {
      response.writeHead(200, { "Content-Type": "text/event-stream" });
      // Deliberately split mid-event, which is what a real network does.
      response.write("event: ready\ndata: {}\n\nevent: del");
      await new Promise((resolve) => setTimeout(resolve, 10));
      response.write(`ta\ndata: ${payload}\n`);
      await new Promise((resolve) => setTimeout(resolve, 10));
      response.write('\nevent: complete\ndata: {"delta_count":1}\n\n');
      response.end();
    });
  });
  servers.push(server);
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("server has no port");

  const chunks = await collect(`http://127.0.0.1:${address.port}/solve`);

  assert.equal(chunks.length, 1);
  assert.deepEqual(chunks[0], delta(10n, 7, 0, 1.5));
});

test("throws the solver's error code", async () => {
  const { url } = await serve((write) => {
    write("error", '{"code":"invalid_input","message":"required property absent"}');
  });

  await assert.rejects(collect(url), (error: SolverFailed) => {
    assert.equal(error.code, "invalid_input");
    assert.match(error.message, /required property absent/);
    return true;
  });
});

test("treats a stream that ends without complete as failed", async () => {
  const { url } = await serve((write) => {
    write("ready", "{}");
    write("delta", base64(delta(10n, 7, 0, 1)));
  });

  await assert.rejects(collect(url), /stream ended without complete/);
});

test("stops at an error even after deltas have arrived", async () => {
  const { url } = await serve((write) => {
    write("ready", "{}");
    write("delta", base64(delta(10n, 7, 0, 1)));
    write("error", '{"code":"execution_failed","message":"diverged"}');
  });

  await assert.rejects(collect(url), (error: SolverFailed) => error.code === "execution_failed");
});
