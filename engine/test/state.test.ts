// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

import { decompress } from "fzstd";

import { parseContainer } from "../src/container.js";
import { readSection } from "../src/sections.js";
import { parseStateChannels, StateRing, type StateChannel } from "../src/state.js";

const bytes = readFileSync(new URL("../../fixtures/network.trama", import.meta.url));
const file = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);

/** Builds an STCH payload the way SPEC section 6 describes it. */
function stchPayload(channels: readonly Partial<StateChannel>[], names: readonly string[]): Uint8Array {
  const encoder = new TextEncoder();
  const encoded = names.map((name) => encoder.encode(name));
  const stringBytes = 4 + encoded.reduce((total, name) => total + 4 + name.byteLength, 0);
  const payload = new Uint8Array(12 + stringBytes + channels.length * 24);
  const view = new DataView(payload.buffer);
  view.setUint32(0, channels.length, true);
  view.setUint32(4, 12, true);
  view.setUint32(8, 12 + stringBytes, true);
  view.setUint32(12, encoded.length, true);
  let at = 16;
  for (const name of encoded) {
    view.setUint32(at, name.byteLength, true);
    payload.set(name, at + 4);
    at += 4 + name.byteLength;
  }
  channels.forEach((channel, index) => {
    const base = 12 + stringBytes + index * 24;
    view.setUint16(base, channel.channelId ?? index + 1, true);
    view.setUint8(base + 2, channel.entityKind ?? 2);
    view.setUint8(base + 3, 1);
    view.setUint32(base + 4, index, true);
    view.setUint32(base + 8, index, true);
    view.setFloat32(base + 12, channel.declaredMin ?? 0, true);
    view.setFloat32(base + 16, channel.declaredMax ?? 0, true);
    view.setUint32(base + 20, (channel.rangePresent ? 1 : 0) | (channel.linearInterpolation ? 2 : 0), true);
  });
  return payload;
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

const pressure: StateChannel = {
  channelId: 7,
  entityKind: 2,
  name: "pressure",
  unit: "m",
  declaredMin: 0,
  declaredMax: 100,
  rangePresent: true,
  linearInterpolation: true,
};

const ringOptions = { channels: [pressure], nodeIds: [], edgeIds: [10n, 20n], slots: 3, slotSeconds: 60 };

test("reads the STCH the compiler writes", () => {
  const container = parseContainer(file);
  const section = container.sections.find((candidate) => candidate.type === "STCH")!;

  const channels = parseStateChannels(readSection(file, section, (stored) => decompress(stored)));

  assert.deepEqual(channels, []);
});

test("resolves channel names and units through the string table", () => {
  const channels = parseStateChannels(
    stchPayload([{ channelId: 7, entityKind: 2, declaredMin: 0, declaredMax: 100, rangePresent: true }], ["pressure"]),
  );

  assert.equal(channels[0]?.name, "pressure");
  assert.equal(channels[0]?.entityKind, 2);
  assert.equal(channels[0]?.rangePresent, true);
});

test("rejects a channel id of zero", () => {
  assert.throws(() => parseStateChannels(stchPayload([{ channelId: 0 }], ["x"])), /non-zero/);
});

test("rejects an inverted declared range", () => {
  assert.throws(
    () => parseStateChannels(stchPayload([{ declaredMin: 10, declaredMax: 1, rangePresent: true }], ["x"])),
    /inverted range/,
  );
});

test("writes a delta at the column of its entity", () => {
  const ring = new StateRing(ringOptions);

  assert.equal(ring.apply(delta(20n, 7, 0, 42)), 1);

  assert.equal(ring.texels[1], 42);
  assert.equal(ring.texels[0], 0);
});

test("wraps around its slots as time advances", () => {
  const ring = new StateRing(ringOptions);

  ring.apply(delta(10n, 7, 0, 1));
  ring.apply(delta(10n, 7, 180, 4));

  // 0 s and 180 s are three slots apart, so the later sample overwrites the earlier one.
  assert.equal(ring.slotFor(0), ring.slotFor(180));
  assert.equal(ring.texels[0], 4);
  assert.equal(ring.slotSeconds[0], 180);
});

test("clears a slot it is reusing rather than leaving a stale entity behind", () => {
  const ring = new StateRing(ringOptions);

  ring.apply(delta(10n, 7, 0, 1));
  ring.apply(delta(20n, 7, 0, 2));
  ring.apply(delta(10n, 7, 180, 9));

  assert.equal(ring.texels[0], 9);
  assert.equal(ring.texels[1], 0, "the previous occupant of this slot must not survive");
});

test("rejects a stream whose length is not a multiple of 18", () => {
  const ring = new StateRing(ringOptions);

  assert.throws(() => ring.apply(new Uint8Array(19)), /multiple of 18 bytes/);
});

test("rejects an undeclared channel", () => {
  const ring = new StateRing(ringOptions);

  assert.throws(() => ring.apply(delta(10n, 99, 0, 1)), /undeclared channel 99/);
});

test("rejects an entity the graph does not hold", () => {
  const ring = new StateRing(ringOptions);

  assert.throws(() => ring.apply(delta(999n, 7, 0, 1)), /entity 999, which no edge has/);
});

test("rejects a node id written to an edge channel", () => {
  const ring = new StateRing({ ...ringOptions, nodeIds: [10n], edgeIds: [20n] });

  assert.throws(() => ring.apply(delta(10n, 7, 0, 1)), /which no edge has/);
});

test("rejects a value outside a declared range", () => {
  const ring = new StateRing(ringOptions);

  assert.throws(() => ring.apply(delta(10n, 7, 0, 101)), /outside the declared range of pressure \[0, 100\]/);
});

test("accepts any finite value when the channel declares no range", () => {
  const ring = new StateRing({ ...ringOptions, channels: [{ ...pressure, rangePresent: false }] });

  ring.apply(delta(10n, 7, 0, 1e6));

  assert.equal(ring.texels[0], 1e6);
});

test("rejects a non-finite value", () => {
  const ring = new StateRing({ ...ringOptions, channels: [{ ...pressure, rangePresent: false }] });

  assert.throws(() => ring.apply(delta(10n, 7, 0, Number.NaN)), /is not finite/);
});

test("leaves the ring untouched when any record in the stream is invalid", () => {
  const ring = new StateRing(ringOptions);
  const stream = new Uint8Array(36);
  stream.set(delta(10n, 7, 0, 5), 0);
  stream.set(delta(10n, 7, 0, 500), 18);

  assert.throws(() => ring.apply(stream), /outside the declared range/);
  assert.equal(ring.texels[0], 0, "a rejected stream must not be half-applied");
});
