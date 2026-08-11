// SPDX-License-Identifier: LicenseRef-BSL-1.1
/**
 * The playground offline.
 *
 * OPFS caches what a container is made of; this caches what reads it. Between them the page
 * needs a network exactly once — which is the first pillar's promise, and was untrue until both
 * existed.
 *
 * `build.sh` writes `sw.js` from this file, filling in the asset list it just produced and a
 * version derived from their bytes. The list cannot be written by hand: whether the EPANET
 * module is there at all depends on whether the build had a WASI SDK.
 */
const VERSION = "__VERSION__";
const ASSETS = __ASSETS__;
const CACHE = `trama-playground-${VERSION}`;

self.addEventListener("install", (event) => {
  // All of it, up front: a partial precache is an offline page that fails halfway through a
  // task, which is worse than one that never claimed to work.
  event.waitUntil(caches.open(CACHE).then((cache) => cache.addAll(ASSETS)).then(() => self.skipWaiting()));
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((names) => Promise.all(names.filter((name) => name !== CACHE).map((name) => caches.delete(name))))
      .then(() => self.clients.claim()),
  );
});

self.addEventListener("fetch", (event) => {
  const request = event.request;
  // Only this origin's own GETs. A solver on another host is a live thing to talk to, not an
  // asset, and serving it from a cache would return yesterday's answer to today's question.
  if (request.method !== "GET" || new URL(request.url).origin !== self.location.origin) return;
  event.respondWith(
    caches.match(request, { ignoreSearch: true }).then((hit) => {
      if (hit !== undefined) return hit;
      // Cache-first, then network. Each build has its own cache name and the old one is deleted
      // on activate, so nothing here can pin a stale asset across a deploy.
      return fetch(request).then((response) => {
        if (response.ok && response.type === "basic") {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy));
        }
        return response;
      });
    }),
  );
});
