// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import test from "node:test";

import { createStateTexture } from "../src/state-texture.js";
import { StateRing, type StateChannel } from "../src/state.js";

const channel: StateChannel = {
  channelId: 7,
  entityKind: 2,
  name: "pressure",
  unit: "m",
  declaredMin: 0,
  declaredMax: 100,
  rangePresent: true,
  linearInterpolation: true,
};

const constants = {
  TEXTURE_2D: 0x0de1,
  R32F: 0x822e,
  RED: 0x1903,
  FLOAT: 0x1406,
  NEAREST: 0x2600,
  TEXTURE_MIN_FILTER: 0x2801,
  TEXTURE_MAG_FILTER: 0x2800,
  TEXTURE_WRAP_S: 0x2802,
  TEXTURE_WRAP_T: 0x2803,
  CLAMP_TO_EDGE: 0x812f,
};

function recordingContext() {
  const calls: { name: string; args: unknown[] }[] = [];
  const record =
    (name: string, result?: unknown) =>
    (...args: unknown[]) => {
      calls.push({ name, args });
      return result;
    };
  const gl = {
    ...constants,
    createTexture: record("createTexture", "texture"),
    bindTexture: record("bindTexture"),
    texImage2D: record("texImage2D"),
    texParameteri: record("texParameteri"),
    deleteTexture: record("deleteTexture"),
  };
  return { gl: gl as unknown as WebGL2RenderingContext, calls };
}

const ring = new StateRing({ channels: [channel], nodeIds: [], edgeIds: [10n, 20n], slots: 3, slotSeconds: 60 });

test("uploads the ring as a single-channel float texture", () => {
  const { gl, calls } = recordingContext();

  createStateTexture(gl, ring);

  assert.deepEqual(calls.find((call) => call.name === "texImage2D")?.args, [
    constants.TEXTURE_2D,
    0,
    constants.R32F,
    ring.width,
    ring.height,
    0,
    constants.RED,
    constants.FLOAT,
    ring.texels,
  ]);
});

test("samples without filtering, so the shader owns the interpolation", () => {
  const { gl, calls } = recordingContext();

  createStateTexture(gl, ring);

  const filters = calls
    .filter((call) => call.name === "texParameteri")
    .filter((call) => call.args[1] === constants.TEXTURE_MIN_FILTER || call.args[1] === constants.TEXTURE_MAG_FILTER);
  assert.equal(filters.length, 2);
  assert.ok(filters.every((call) => call.args[2] === constants.NEAREST));
});

test("re-uploads on update and releases on dispose", () => {
  const { gl, calls } = recordingContext();
  const texture = createStateTexture(gl, ring);

  texture.update(ring);
  texture.dispose();

  assert.equal(calls.filter((call) => call.name === "texImage2D").length, 2);
  assert.equal(calls.filter((call) => call.name === "deleteTexture").length, 1);
});
