// SPDX-License-Identifier: LicenseRef-BSL-1.1

export type StateChannel = Readonly<{
  channelId: number;
  /** 1 = node, 2 = edge. */
  entityKind: number;
  name: string;
  unit: string;
  declaredMin: number;
  declaredMax: number;
  rangePresent: boolean;
  linearInterpolation: boolean;
}>;

const CHANNEL_BYTES = 24;
const DELTA_BYTES = 18;

/** Reads STCH per SPEC section 6. The file declares what solvers may write; it holds no samples. */
export function parseStateChannels(payload: Uint8Array): readonly StateChannel[] {
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  if (payload.byteLength < 12) throw new Error("STCH is shorter than its header");
  const channelCount = view.getUint32(0, true);
  const stringsOffset = view.getUint32(4, true);
  const channelsOffset = view.getUint32(8, true);
  if (channelsOffset + channelCount * CHANNEL_BYTES > payload.byteLength) {
    throw new Error("STCH channels exceed payload bounds");
  }
  const strings = readStrings(view, payload, stringsOffset);

  return Array.from({ length: channelCount }, (_, index) => {
    const at = channelsOffset + index * CHANNEL_BYTES;
    const valueType = view.getUint8(at + 3);
    if (valueType !== 1) throw new Error("v0 state channels must be scalar f32");
    const flags = view.getUint32(at + 20, true);
    const channel: StateChannel = {
      channelId: view.getUint16(at, true),
      entityKind: view.getUint8(at + 2),
      name: stringAt(strings, view.getUint32(at + 4, true)),
      unit: stringAt(strings, view.getUint32(at + 8, true)),
      declaredMin: view.getFloat32(at + 12, true),
      declaredMax: view.getFloat32(at + 16, true),
      rangePresent: (flags & 1) !== 0,
      linearInterpolation: (flags & 2) !== 0,
    };
    if (channel.channelId === 0) throw new Error("a state channel id must be non-zero");
    if (channel.rangePresent && channel.declaredMin > channel.declaredMax) {
      throw new Error(`channel ${channel.name} declares an inverted range`);
    }
    return channel;
  });
}

export type StateRingOptions = Readonly<{
  channels: readonly StateChannel[];
  /** Entity IDs in array order, per kind, as GRPH stores them. */
  nodeIds: readonly bigint[];
  edgeIds: readonly bigint[];
  /** Time slots retained. Scrubbing back further than this is a cache miss, not an error. */
  slots: number;
  /** Seconds between slots. */
  slotSeconds: number;
}>;

/**
 * The temporal ring behind video-style scrubbing. One column per entity, one row per
 * (slot, channel), so the whole thing uploads as a texture indexed by entity index.
 */
export class StateRing {
  readonly texels: Float32Array;
  readonly width: number;
  readonly height: number;
  /** The time each slot currently holds, or NaN while it has never been written. */
  readonly slotSeconds: Float32Array;

  readonly #channels: Map<number, { channel: StateChannel; row: number }>;
  readonly #entities: Map<number, Map<bigint, number>>;
  readonly #slots: number;
  readonly #step: number;

  constructor(options: StateRingOptions) {
    if (options.slots < 1) throw new Error("a state ring needs at least one slot");
    if (!(options.slotSeconds > 0)) throw new Error("a state ring needs a positive slot duration");
    this.#slots = options.slots;
    this.#step = options.slotSeconds;
    this.#channels = new Map(
      options.channels.map((channel, row) => [channel.channelId, { channel, row }]),
    );
    this.#entities = new Map([
      [1, new Map(options.nodeIds.map((id, index) => [id, index]))],
      [2, new Map(options.edgeIds.map((id, index) => [id, index]))],
    ]);
    // ponytail: one texture wide enough for the larger entity kind, so node channels waste
    // columns when edges outnumber nodes. Split into a texture per kind if that waste shows up.
    this.width = Math.max(options.nodeIds.length, options.edgeIds.length);
    this.height = options.slots * options.channels.length;
    this.texels = new Float32Array(this.width * this.height);
    this.slotSeconds = new Float32Array(options.slots).fill(Number.NaN);
  }

  /** The slot holding time `t`. Older samples are overwritten as time advances. */
  slotFor(t: number): number {
    return ((Math.floor(t / this.#step) % this.#slots) + this.#slots) % this.#slots;
  }

  /**
   * Applies a packed delta stream. Every record is validated before any texel is written, so a
   * rejected stream leaves the ring untouched rather than half-applied.
   */
  apply(deltas: Uint8Array): number {
    if (deltas.byteLength % DELTA_BYTES !== 0) {
      throw new Error(`a delta stream must be a multiple of ${DELTA_BYTES} bytes`);
    }
    const view = new DataView(deltas.buffer, deltas.byteOffset, deltas.byteLength);
    const writes = Array.from({ length: deltas.byteLength / DELTA_BYTES }, (_, index) =>
      this.#validate(view, index * DELTA_BYTES),
    );
    for (const write of writes) {
      const slot = this.slotFor(write.t);
      if (!Object.is(this.slotSeconds[slot], write.slotTime)) {
        this.#clearSlot(slot);
        this.slotSeconds[slot] = write.slotTime;
      }
      this.texels[(slot * this.#channels.size + write.row) * this.width + write.column] = write.value;
    }
    return writes.length;
  }

  #validate(view: DataView, at: number) {
    const entityId = view.getBigUint64(at, true);
    const channelId = view.getUint16(at + 8, true);
    const t = view.getFloat32(at + 10, true);
    const value = view.getFloat32(at + 14, true);
    const found = this.#channels.get(channelId);
    if (found === undefined) throw new Error(`delta names undeclared channel ${channelId}`);
    if (!Number.isFinite(t) || !Number.isFinite(value)) {
      throw new Error(`delta for channel ${found.channel.name} is not finite`);
    }
    if (found.channel.rangePresent && (value < found.channel.declaredMin || value > found.channel.declaredMax)) {
      throw new Error(
        `delta ${value} is outside the declared range of ${found.channel.name} ` +
          `[${found.channel.declaredMin}, ${found.channel.declaredMax}]`,
      );
    }
    const column = this.#entities.get(found.channel.entityKind)?.get(entityId);
    if (column === undefined) {
      throw new Error(`delta names entity ${entityId}, which no ${kindName(found.channel.entityKind)} has`);
    }
    return { row: found.row, column, value, t, slotTime: Math.floor(t / this.#step) * this.#step };
  }

  #clearSlot(slot: number): void {
    const start = slot * this.#channels.size * this.width;
    this.texels.fill(0, start, start + this.#channels.size * this.width);
  }
}

function kindName(entityKind: number): string {
  return entityKind === 1 ? "node" : entityKind === 2 ? "edge" : `entity kind ${entityKind}`;
}

function readStrings(view: DataView, payload: Uint8Array, offset: number): readonly string[] {
  if (offset + 4 > payload.byteLength) throw new Error("STCH string table exceeds payload bounds");
  const count = view.getUint32(offset, true);
  const decoder = new TextDecoder();
  const values: string[] = [];
  let at = offset + 4;
  for (let index = 0; index < count; index += 1) {
    if (at + 4 > payload.byteLength) throw new Error("STCH string table exceeds payload bounds");
    const length = view.getUint32(at, true);
    if (at + 4 + length > payload.byteLength) throw new Error("STCH string table exceeds payload bounds");
    values.push(decoder.decode(payload.subarray(at + 4, at + 4 + length)));
    at += 4 + length;
  }
  return values;
}

function stringAt(strings: readonly string[], index: number): string {
  const value = strings[index];
  if (value === undefined) throw new Error(`STCH names string ${index}, which the table does not hold`);
  return value;
}
