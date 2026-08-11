// SPDX-License-Identifier: LicenseRef-BSL-1.1
/**
 * Frame-time benchmark for the phase 4 criterion: 100k segments with animated state.
 *
 *   uv run --project ../compiler python ../compiler/benchmarks/grid_container.py --side 224 --out /tmp/bench.trama
 *   node bench/frames.mjs --container /tmp/bench.trama
 *
 * The budget is the environment's: FRAME_BUDGET_MS, 16.7 by default. A machine without a GPU
 * renders through SwiftShader and will not meet it — that is a fact about the machine, so the
 * exit code is a failure only when a budget was asked for explicitly.
 */

import { createReadStream, statSync } from "node:fs";
import { createServer } from "node:http";
import { extname, join, resolve } from "node:path";
import { chromium } from "playwright";

const root = resolve(import.meta.dirname, "..", "..");
const argument = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at === -1 ? fallback : process.argv[at + 1];
};

const container = resolve(argument("container", "/tmp/bench.trama"));
const budget = Number(process.env.FRAME_BUDGET_MS ?? 16.7);
const enforce = process.env.FRAME_BUDGET_MS !== undefined;

const TYPES = { ".html": "text/html", ".mjs": "text/javascript", ".js": "text/javascript", ".trama": "application/octet-stream" };

const server = createServer((request, response) => {
  const path = request.url.split("?")[0];
  const file = path === "/bench-container.trama" ? container : join(root, path === "/" ? "/engine/bench/index.html" : path);
  try {
    statSync(file);
  } catch {
    response.writeHead(404).end();
    return;
  }
  response.writeHead(200, { "Content-Type": TYPES[extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(response);
});

await new Promise((ready) => server.listen(0, ready));
const port = server.address().port;

const browser = await chromium.launch({ args: ["--enable-gpu", "--ignore-gpu-blocklist"] });
const page = await browser.newPage({ viewport: { width: 1280, height: 720 } });
const failures = [];
page.on("pageerror", (error) => failures.push(error.message));
await page.goto(`http://127.0.0.1:${port}/engine/bench/index.html`);
const result = await page.waitForFunction(() => window.__bench, null, { timeout: 120000 }).then((handle) => handle.jsonValue());
// A benchmark that draws nothing is very fast. --screenshot is how that stays disprovable.
const shot = argument("screenshot", null);
if (shot !== null) await page.screenshot({ path: shot });
await browser.close();
server.close();

if (result.error || failures.length > 0) {
  console.error(result.error ?? failures.join("\n"));
  process.exit(1);
}

if (result.stateFrames === 0) {
  console.error("no frame bound the state texture: this would be measuring the flat-colour path");
  process.exit(1);
}

const verdict = result.p95 <= budget ? "PASS" : "FAIL";
console.log(`${result.segments} segments in ${result.tiles} tiles, ${result.edges} edges`);
console.log(`renderer  ${result.renderer}`);
console.log(`state     ${result.stateMilliseconds.toFixed(0)} ms to fill 16 slots, bound on ${result.stateFrames}/${result.frames} frames`);
console.log(`frame p50 ${result.p50.toFixed(2)} ms`);
console.log(`frame p95 ${result.p95.toFixed(2)} ms   ${verdict} (budget ${budget} ms)`);
console.log(`worst     ${result.worst.toFixed(2)} ms   over ${result.frames} frames`);
console.log(`cadence   ${result.cadence.toFixed(2)} ms   ${result.late} frames arrived late`);

if (enforce && verdict === "FAIL") process.exit(1);
