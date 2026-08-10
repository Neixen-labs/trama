// SPDX-License-Identifier: LicenseRef-BSL-1.1
import { directoryRange, HEADER_RANGE, parsePrefix, type Container, type Section } from "./container.js";
import { crc32c, type Decompress } from "./sections.js";

/** Fetches `[start, endInclusive]`. Both ends are inclusive, matching HTTP's Range header. */
export type RangeReader = (start: number, endInclusive: number) => Promise<Uint8Array>;

/**
 * Reads the header, then the directory, and nothing else — the access pattern the format's
 * absolute offsets exist for. Two round trips, because the section count is in the header.
 */
export async function openContainer(read: RangeReader): Promise<Container> {
  const header = await read(...HEADER_RANGE);
  const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
  if (header.byteLength < 64) throw new Error("invalid TRAMA header");
  const sectionCount = view.getUint32(32, true);
  const fileBytes = Number(view.getBigUint64(40, true));
  const prefix = await read(...directoryRange(sectionCount));
  // A copy into a plain ArrayBuffer: a reader may hand back a view onto a SharedArrayBuffer.
  const buffer = new ArrayBuffer(prefix.byteLength);
  new Uint8Array(buffer).set(prefix);
  return parsePrefix(buffer, fileBytes);
}

/** Fetches one section and applies the same verification a local read gets. */
export async function fetchSection(read: RangeReader, section: Section, decompress: Decompress): Promise<Uint8Array> {
  if (section.codec !== 1) throw new Error("unsupported section codec");
  const start = Number(section.offset);
  const stored = await read(start, start + Number(section.storedBytes) - 1);
  if (stored.byteLength !== Number(section.storedBytes)) throw new Error("range reader returned the wrong length");
  const payload = decompress(stored, Number(section.uncompressedBytes));
  if (payload.byteLength !== Number(section.uncompressedBytes)) throw new Error("section decoded length mismatch");
  if (crc32c(payload) !== section.crc32c) throw new Error("section checksum mismatch");
  return payload;
}

/**
 * A reader backed by HTTP range requests. A `200` means the server ignored the Range header and
 * sent the whole object, which defeats the point, so it is rejected rather than quietly accepted.
 */
export function httpRangeReader(url: string, request: typeof fetch = fetch): RangeReader {
  return async (start, endInclusive) => {
    const response = await request(url, { headers: { Range: `bytes=${start}-${endInclusive}` } });
    if (response.status !== 206) {
      throw new Error(`expected 206 for a range request, got ${response.status}`);
    }
    return new Uint8Array(await response.arrayBuffer());
  };
}
