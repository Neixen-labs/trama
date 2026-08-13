// SPDX-License-Identifier: LicenseRef-BSL-1.1
/**
 * The phase 5 criterion, timed: a stranger arrives, drops their own `.inp`, and sees their
 * network simulated in under 60 seconds.
 *
 *   node bench/journey.mjs --file ../core/trama-epanet/tests/networks/Net3.inp
 *   node bench/journey.mjs --base http://localhost:8080/demo/ --file mine.inp
 *
 * It measures the deployed playground by default, because that is where the stranger lands and
 * the network is part of what they wait for. A cold profile every run: no service worker, no
 * HTTP cache, which is the first visit and the only one the criterion is about.
 */

import { resolve } from "node:path";
import { chromium } from "playwright";

const argument = (name, fallback) => {
  const at = process.argv.indexOf(`--${name}`);
  return at === -1 ? fallback : process.argv[at + 1];
};

const base = argument("base", "https://trama.build/demo/");
const file = resolve(argument("file", "../core/trama-epanet/tests/networks/Net3.inp"));
const solver = argument("solver", "epanet");
const crs = argument("crs", null);
const budget = Number(argument("budget", 60));

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 1280, height: 800 } });
const page = await context.newPage();

// A number from a desktop on fibre answers a question nobody asked. `--slow` is the stranger
// the criterion is about: a mid-range phone on mobile data, which is four times the CPU cost
// and a link that makes 400 kB of compiler a wait rather than a rounding error.
if (process.argv.includes("--slow")) {
  const cdp = await context.newCDPSession(page);
  await cdp.send("Emulation.setCPUThrottlingRate", { rate: 4 });
  await cdp.send("Network.emulateNetworkConditions", {
    offline: false,
    latency: 150,
    downloadThroughput: (1.6 * 1024 * 1024) / 8,
    uploadThroughput: (750 * 1024) / 8,
  });
}
const failures = [];
page.on("pageerror", (error) => failures.push(error.message));

const marks = [];
const started = performance.now();
const mark = (what) => marks.push({ what, at: (performance.now() - started) / 1000 });

try {
  await page.goto(base, { waitUntil: "commit" });
  // Ready is the examples becoming clickable: the page renders long before the compiler that
  // makes it useful has arrived, so anything earlier would be timing an empty panel.
  await page.waitForSelector("[data-example]:not([disabled])", { timeout: 60000 });
  mark("the playground is ready");

  if (crs !== null) await page.fill("#crs", crs);
  await page.setInputFiles("#file", file);
  await page.waitForFunction(
    () => /compilado en|contenedor/.test(document.querySelector("#stats").textContent),
    null,
    { timeout: 60000 },
  );
  mark("the network is compiled and on the map");

  const offered = await page.$$eval("#engine-choice option", (options) => options.map((option) => option.value));
  if (!offered.includes(solver)) throw new Error(`the page offers ${offered.join(", ")}, not ${solver}`);
  await page.selectOption("#engine-choice", solver);
  await page.click("#solve");
  // Enabling the scrub is the page's own statement that state reached the texture: it is set
  // one line after `refreshState`, so there is a painted frame behind it.
  await page.waitForSelector("#scrub:not([disabled])", { timeout: 120000 });
  mark("state is on the network and time can be scrubbed");

  // Scrubbing is the thing the criterion promises to a stranger, so the run does it rather
  // than trusting an enabled slider.
  await page.$eval("#scrub", (slider) => {
    slider.value = String(Math.round(Number(slider.max) / 2));
    slider.dispatchEvent(new Event("input", { bubbles: true }));
  });
  mark("time scrubbed to the middle of the window");

  const shot = argument("screenshot", null);
  if (shot !== null) await page.screenshot({ path: shot });

  const summary = await page.$eval("#stats", (list) =>
    [...list.children].reduce((pairs, child, index, all) => {
      if (child.tagName === "DT") pairs.push(`${child.textContent.trim()}: ${all[index + 1]?.textContent.trim()}`);
      return pairs;
    }, []),
  );

  // On a slow link the whole run is dominated by bytes rather than by the network's size, so
  // the report says how many arrived and which single file was the largest.
  const transfer = await page.evaluate(() => {
    const resources = performance.getEntriesByType("resource");
    const largest = resources.reduce((worst, entry) => (entry.transferSize > worst.transferSize ? entry : worst));
    return {
      files: resources.length,
      kilobytes: Math.round(resources.reduce((total, entry) => total + entry.transferSize, 0) / 1024),
      largest: `${largest.name.split("/").pop()} at ${Math.round(largest.transferSize / 1024)} kB`,
    };
  });

  const total = marks.at(-1).at;
  console.log(`${base}\n${file}\n`);
  let previous = 0;
  for (const { what, at } of marks) {
    console.log(`${at.toFixed(1).padStart(6)} s  (+${(at - previous).toFixed(1)} s)  ${what}`);
    previous = at;
  }
  console.log(`\n${summary.join("\n")}`);
  console.log(`fetched: ${transfer.files} files, ${transfer.kilobytes} kB, largest ${transfer.largest}`);
  console.log(`\ntotal ${total.toFixed(1)} s   ${total <= budget ? "PASS" : "FAIL"} (criterion ${budget} s)`);
  if (failures.length > 0) console.error(`\npage errors:\n${failures.join("\n")}`);
  process.exitCode = total <= budget && failures.length === 0 ? 0 : 1;
} finally {
  await browser.close();
}
