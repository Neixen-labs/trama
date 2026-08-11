// SPDX-License-Identifier: LicenseRef-BSL-1.1
/**
 * Range reads kept in the origin private file system, so a container fetched once is readable
 * without a network.
 *
 * The format is what makes this worth doing: a reader asks for the header, the directory and the
 * tiles it can see, so what lands in the cache is what was actually looked at rather than a whole
 * file downloaded on the chance it might be.
 */
import type { RangeReader } from "./range.js";

/** The part of `navigator.storage` this needs, named so a test can supply its own. */
export type OpfsStorage = Readonly<{ getDirectory(): Promise<FileSystemDirectoryHandle> }>;

export type CacheOptions = Readonly<{
  /**
   * Identifies the container. Ranges are stored under it, so two containers never mix — and a
   * container whose bytes change under the same key would be served the old ones.
   *
   * ponytail: no revalidation. Nothing is fetched to check freshness, which is the whole point
   * offline; the cost is that a republished file needs a new key or `forget`. HTTP validators
   * would need a request, and a `.trama` is meant to be immutable — SPEC 10 says a new dataset
   * version is a new file.
   */
  key: string;
  storage?: OpfsStorage | undefined;
}>;

const ROOT = "trama-cache";

/**
 * Wraps a reader so each range is written to OPFS on the way through and read from it after.
 *
 * Degrades to the reader it wraps wherever OPFS is missing — an insecure context, a browser
 * without it, Node — because a cache that throws is worse than a cache that does nothing.
 */
export function cachedInOpfs(inner: RangeReader, options: CacheOptions): RangeReader {
  const storage = options.storage ?? (globalThis.navigator?.storage as OpfsStorage | undefined);
  const directory = openDirectory(storage, options.key);
  return async (start, endInclusive) => {
    const folder = await directory;
    if (folder === null) return inner(start, endInclusive);
    const name = `${start}-${endInclusive}`;
    const hit = await read(folder, name);
    if (hit !== null) return hit;
    const fresh = await inner(start, endInclusive);
    await write(folder, name, fresh);
    return fresh;
  };
}

/** Drops everything cached for one key. The way to re-read a file republished at the same URL. */
export async function forget(key: string, storage?: OpfsStorage): Promise<void> {
  const resolved = storage ?? (globalThis.navigator?.storage as OpfsStorage | undefined);
  if (resolved === undefined) return;
  try {
    const root = await resolved.getDirectory();
    const cache = await root.getDirectoryHandle(ROOT);
    await cache.removeEntry(safeName(key), { recursive: true });
  } catch {
    // Nothing cached under that key, which is the state the caller asked for.
  }
}

async function openDirectory(storage: OpfsStorage | undefined, key: string): Promise<FileSystemDirectoryHandle | null> {
  if (storage === undefined || typeof storage.getDirectory !== "function") return null;
  try {
    const root = await storage.getDirectory();
    const cache = await root.getDirectoryHandle(ROOT, { create: true });
    return await cache.getDirectoryHandle(safeName(key), { create: true });
  } catch {
    // Storage can be denied outright, and a container that only loads with a cache would be a
    // worse product than one that loads slowly.
    return null;
  }
}

async function read(folder: FileSystemDirectoryHandle, name: string): Promise<Uint8Array | null> {
  try {
    const handle = await folder.getFileHandle(name);
    const file = await handle.getFile();
    return new Uint8Array(await file.arrayBuffer());
  } catch {
    return null;
  }
}

async function write(folder: FileSystemDirectoryHandle, name: string, bytes: Uint8Array): Promise<void> {
  try {
    const handle = await folder.getFileHandle(name, { create: true });
    const writable = await handle.createWritable();
    // A copy into a plain buffer: a reader may hand back a view onto a SharedArrayBuffer, which
    // this API will not take. `range.ts` copies for the same reason.
    const chunk = new Uint8Array(bytes.byteLength);
    chunk.set(bytes);
    await writable.write(chunk);
    await writable.close();
  } catch {
    // A full or denied disk must not fail the read that already succeeded.
  }
}

/** A directory name from an arbitrary key: OPFS accepts most characters, `/` not among them. */
function safeName(key: string): string {
  return key.replace(/[^a-zA-Z0-9._-]/g, "_").slice(0, 120) || "unnamed";
}
