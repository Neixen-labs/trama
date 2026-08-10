// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import test from "node:test";

import { parseContainer } from "../src/container.js";

function fixture(): ArrayBuffer {
  const bytes = new ArrayBuffer(128);
  const view = new DataView(bytes);
  new Uint8Array(bytes, 0, 8).set(new TextEncoder().encode("TRAMA\0\0\0"));
  view.setUint16(8, 0, true);
  view.setUint16(10, 1, true);
  view.setUint32(20, 64, true);
  view.setBigUint64(24, 64n, true);
  view.setUint32(32, 1, true);
  view.setBigUint64(40, 128n, true);
  new Uint8Array(bytes, 64, 4).set(new TextEncoder().encode("GEOM"));
  view.setUint32(68, 1, true);
  view.setUint32(72, 14, true);
  view.setBigUint64(84, 128n, true);
  return bytes;
}

test("parses a v0 header and directory", () => {
  const container = parseContainer(fixture());

  assert.deepEqual(container.version, [0, 1, 0]);
  assert.equal(container.sections[0]?.type, "GEOM");
  assert.deepEqual(container.sections[0]?.key, [14, 0, 0]);
});

test("rejects a section outside the container", () => {
  const bytes = fixture();
  new DataView(bytes).setBigUint64(84, 129n, true);

  assert.throws(() => parseContainer(bytes), /section exceeds file bounds/);
});
