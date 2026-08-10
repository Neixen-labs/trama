// SPDX-License-Identifier: LicenseRef-BSL-1.1
export type Section = Readonly<{
  type: string;
  required: boolean;
  key: readonly [number, number, number];
  offset: bigint;
  storedBytes: bigint;
  uncompressedBytes: bigint;
  crc32c: number;
  codec: number;
}>;

export type Container = Readonly<{
  version: readonly [number, number, number];
  minimumReaderVersion: readonly [number, number, number];
  sections: readonly Section[];
}>;

const HEADER_BYTES = 64;
const DIRECTORY_BYTES = 64;
const MAGIC = "TRAMA\0\0\0";

export function parseContainer(bytes: ArrayBuffer): Container {
  const view = new DataView(bytes);
  if (bytes.byteLength < HEADER_BYTES || new TextDecoder().decode(bytes.slice(0, 8)) !== MAGIC) {
    throw new Error("invalid TRAMA header");
  }
  const version = tuple3(view, 8);
  const minimumReaderVersion = tuple3(view, 14);
  const headerBytes = view.getUint32(20, true);
  const directoryOffset = view.getBigUint64(24, true);
  const sectionCount = view.getUint32(32, true);
  const fileBytes = view.getBigUint64(40, true);
  if (headerBytes !== HEADER_BYTES || directoryOffset !== BigInt(HEADER_BYTES) || fileBytes !== BigInt(bytes.byteLength)) {
    throw new Error("invalid TRAMA header");
  }
  const directoryEnd = HEADER_BYTES + sectionCount * DIRECTORY_BYTES;
  if (directoryEnd > bytes.byteLength) throw new Error("directory exceeds file bounds");

  const sections = Array.from({ length: sectionCount }, (_, index) => parseSection(view, HEADER_BYTES + index * DIRECTORY_BYTES, bytes.byteLength, directoryEnd));
  return { version, minimumReaderVersion, sections };
}

function parseSection(view: DataView, offset: number, fileBytes: number, directoryEnd: number): Section {
  const type = new TextDecoder().decode(new Uint8Array(view.buffer, offset, 4));
  const flags = view.getUint32(offset + 4, true);
  const sectionOffset = view.getBigUint64(offset + 20, true);
  const storedBytes = view.getBigUint64(offset + 28, true);
  if (sectionOffset < BigInt(directoryEnd) || sectionOffset + storedBytes > BigInt(fileBytes)) {
    throw new Error("section exceeds file bounds");
  }
  return {
    type,
    required: (flags & 1) !== 0,
    key: [view.getUint32(offset + 8, true), view.getUint32(offset + 12, true), view.getUint32(offset + 16, true)],
    offset: sectionOffset,
    storedBytes,
    uncompressedBytes: view.getBigUint64(offset + 36, true),
    crc32c: view.getUint32(offset + 44, true),
    codec: view.getUint16(offset + 48, true),
  };
}

function tuple3(view: DataView, offset: number): [number, number, number] {
  return [view.getUint16(offset, true), view.getUint16(offset + 2, true), view.getUint16(offset + 4, true)];
}
