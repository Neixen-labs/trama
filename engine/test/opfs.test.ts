// SPDX-License-Identifier: LicenseRef-BSL-1.1
import assert from "node:assert/strict";
import test from "node:test";

import { cachedInOpfs, forget, type OpfsStorage } from "../src/opfs.js";

/** An in-memory stand-in for OPFS, enough of it for the handful of calls the cache makes. */
function fakeStorage(): OpfsStorage & { files: Map<string, Uint8Array>; failWrites?: boolean } {
  const files = new Map<string, Uint8Array>();
  const state = { files, failWrites: false };
  const directory = (prefix: string): FileSystemDirectoryHandle =>
    ({
      async getDirectoryHandle(name: string, options?: { create?: boolean }) {
        const path = `${prefix}${name}/`;
        if (!options?.create && ![...files.keys()].some((key) => key.startsWith(path))) {
          throw new Error("NotFoundError");
        }
        return directory(path);
      },
      async getFileHandle(name: string, options?: { create?: boolean }) {
        const path = `${prefix}${name}`;
        if (!files.has(path) && !options?.create) throw new Error("NotFoundError");
        return {
          async getFile() {
            const bytes = files.get(path);
            if (bytes === undefined) throw new Error("NotFoundError");
            return { arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) };
          },
          async createWritable() {
            if (state.failWrites) throw new Error("QuotaExceededError");
            return {
              async write(bytes: Uint8Array) {
                files.set(path, bytes);
              },
              async close() {},
            };
          },
        } as unknown as FileSystemFileHandle;
      },
      async removeEntry(name: string) {
        const path = `${prefix}${name}/`;
        for (const key of [...files.keys()]) if (key.startsWith(path)) files.delete(key);
      },
    }) as unknown as FileSystemDirectoryHandle;
  return Object.assign(state, { getDirectory: async () => directory("") });
}

/** A reader that counts how often it was asked, which is the property under test. */
function countingReader() {
  const state = { calls: 0 };
  const read = async (start: number, end: number) => {
    state.calls += 1;
    return Uint8Array.from({ length: end - start + 1 }, (_, index) => (start + index) % 256);
  };
  return { read, state };
}

test("a range is fetched once and served from the cache after", async () => {
  const storage = fakeStorage();
  const inner = countingReader();
  const cached = cachedInOpfs(inner.read, { key: "net.trama", storage });

  const first = await cached(0, 63);
  const second = await cached(0, 63);

  assert.equal(inner.state.calls, 1, "the second read never reached the network");
  assert.deepEqual(second, first);
});

test("different ranges are cached apart", async () => {
  const storage = fakeStorage();
  const inner = countingReader();
  const cached = cachedInOpfs(inner.read, { key: "net.trama", storage });

  await cached(0, 63);
  await cached(64, 127);
  await cached(0, 63);

  assert.equal(inner.state.calls, 2);
});

test("a cached range survives a new reader, which is what offline means", async () => {
  const storage = fakeStorage();
  await cachedInOpfs(countingReader().read, { key: "net.trama", storage })(0, 63);

  const offline = countingReader();
  const bytes = await cachedInOpfs(offline.read, { key: "net.trama", storage })(0, 63);

  assert.equal(offline.state.calls, 0, "nothing was asked of the network at all");
  assert.equal(bytes.byteLength, 64);
});

test("two containers do not read each other's bytes", async () => {
  const storage = fakeStorage();
  const first = countingReader();
  const second = countingReader();

  await cachedInOpfs(first.read, { key: "a.trama", storage })(0, 15);
  await cachedInOpfs(second.read, { key: "b.trama", storage })(0, 15);

  assert.equal(second.state.calls, 1, "the second key is not served the first key's range");
});

test("forget drops one key and leaves the other", async () => {
  const storage = fakeStorage();
  await cachedInOpfs(countingReader().read, { key: "a.trama", storage })(0, 15);
  await cachedInOpfs(countingReader().read, { key: "b.trama", storage })(0, 15);

  await forget("a.trama", storage);

  const a = countingReader();
  const b = countingReader();
  await cachedInOpfs(a.read, { key: "a.trama", storage })(0, 15);
  await cachedInOpfs(b.read, { key: "b.trama", storage })(0, 15);
  assert.equal(a.state.calls, 1, "forgotten, so fetched again");
  assert.equal(b.state.calls, 0, "untouched");
});

test("no OPFS at all means the reader is simply passed through", async () => {
  const inner = countingReader();
  const cached = cachedInOpfs(inner.read, { key: "net.trama", storage: undefined });

  const bytes = await cached(0, 31);

  assert.equal(inner.state.calls, 1);
  assert.equal(bytes.byteLength, 32);
});

test("a storage that refuses to write still returns the bytes", async () => {
  const storage = fakeStorage();
  storage.failWrites = true;
  const inner = countingReader();
  const cached = cachedInOpfs(inner.read, { key: "net.trama", storage });

  const first = await cached(0, 31);
  const second = await cached(0, 31);

  // A full disk degrades to no caching, never to a failed read.
  assert.equal(first.byteLength, 32);
  assert.equal(second.byteLength, 32);
  assert.equal(inner.state.calls, 2);
});

test("keys that are not legal file names still work and stay distinct", async () => {
  const storage = fakeStorage();
  const first = countingReader();
  const second = countingReader();

  await cachedInOpfs(first.read, { key: "https://example.invalid/a.trama", storage })(0, 7);
  await cachedInOpfs(second.read, { key: "https://example.invalid/b.trama", storage })(0, 7);
  const again = countingReader();
  await cachedInOpfs(again.read, { key: "https://example.invalid/a.trama", storage })(0, 7);

  assert.equal(second.state.calls, 1);
  assert.equal(again.state.calls, 0);
});
