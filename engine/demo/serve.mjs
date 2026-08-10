// SPDX-License-Identifier: LicenseRef-BSL-1.1
// Serves the demo with real Range support. file:// cannot range-request, so opening the page
// directly would quietly defeat the very feature this demo exists to show.
import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..", "..");
const port = Number(process.env.PORT ?? 8787);

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".trama": "application/octet-stream",
  ".geojson": "application/geo+json; charset=utf-8",
};

const server = createServer((request, response) => {
  const path = new URL(request.url ?? "/", "http://localhost").pathname;
  const relative = path === "/" ? "engine/demo/index.html" : normalize(path).replace(/^(\.\.[/\\])+/, "");
  const file = join(root, relative);
  if (!file.startsWith(root)) {
    response.writeHead(403).end("forbidden");
    return;
  }

  let stats;
  try {
    stats = statSync(file);
  } catch {
    response.writeHead(404).end(`not found: ${relative}`);
    return;
  }

  const type = TYPES[extname(file)] ?? "application/octet-stream";
  const range = /^bytes=(\d+)-(\d*)$/.exec(request.headers.range ?? "");
  if (range === null) {
    response.writeHead(200, { "Content-Type": type, "Content-Length": stats.size, "Accept-Ranges": "bytes" });
    createReadStream(file).pipe(response);
    return;
  }

  const start = Number(range[1]);
  const end = range[2] === "" ? stats.size - 1 : Math.min(Number(range[2]), stats.size - 1);
  if (start > end) {
    response.writeHead(416, { "Content-Range": `bytes */${stats.size}` }).end();
    return;
  }
  response.writeHead(206, {
    "Content-Type": type,
    "Content-Length": end - start + 1,
    "Content-Range": `bytes ${start}-${end}/${stats.size}`,
  });
  createReadStream(file, { start, end }).pipe(response);
});

server.listen(port, () => {
  console.log(`TRAMA demo on http://localhost:${port}/`);
  console.log("Drop another container in fixtures/ and load it with ?file=<name>.trama");
});
