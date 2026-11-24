// Acceptance part A, round 6: ad-hoc motion probe of one app state.
//
//   node probe.ts '<query>' '<hash>' '<click selector>,…' [sample-ms]
//
// Prints the declared animations/transitions (with the selector of each element), and
// with sample-ms samples the bounding box of the Desk cursor dot and the opacity of the edge
// every 250 ms (T-140).

import { createRequire } from "node:module";
import { existsSync, readFileSync } from "node:fs";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("../../../../../", import.meta.url));
const APP = join(REPO, "apps", "tandem-ui");
const DIST = join(APP, "dist-demo");
const req = createRequire(join(APP, "package.json"));
const { chromium } = req("playwright") as typeof import("playwright");
const [query = "demo", hash = "#/desk", clicks = "", sample = "0"] = process.argv.slice(2);
const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".woff2": "font/woff2", ".woff": "font/woff", ".svg": "image/svg+xml" };

const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 }, colorScheme: "dark" });
const page = await ctx.newPage();
await page.route("**/*", async (route) => {
  const url = new URL(route.request().url());
  if (url.origin !== "http://app.local") return route.abort();
  const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "") || "index.html";
  const path = join(DIST, rel);
  if (!path.startsWith(DIST + sep) || !existsSync(path)) return route.fulfill({ status: 404, body: "" });
  return route.fulfill({ body: readFileSync(path), contentType: TYPES[extname(path)] ?? "application/octet-stream" });
});
await page.goto(`http://app.local/index.html?${query}${hash}`);
await page.waitForSelector("#app > *");
await page.waitForTimeout(800);
for (const c of clicks.split(",").filter(Boolean)) {
  await page.locator(c).first().click();
  await page.waitForTimeout(300);
}
const sigs = await page.evaluate(() => {
  const out = new Set<string>();
  const ms = (d: string) => Math.round(parseFloat(d) * (d.endsWith("ms") ? 1 : 1000));
  const name = (e: Element) => `${e.tagName.toLowerCase()}.${String(e.getAttribute("class") ?? "").split(" ").filter((c) => !c.startsWith("svelte-")).join(".")}`;
  for (const e of Array.from(document.querySelectorAll("body *"))) {
    for (const p of ["", "::before", "::after"]) {
      const s = getComputedStyle(e, p || null);
      if (p && s.content === "none") continue;
      if (s.animationName !== "none") out.add(`anim ${s.animationName.replace(/svelte-[a-z0-9]+-/g, "")} ${s.animationDuration} ${s.animationIterationCount} ${s.animationTimingFunction} <- ${name(e)}${p}`);
      if (s.transitionProperty !== "none" && s.transitionDuration.split(",").some((d) => ms(d.trim()) > 0)) out.add(`trans ${s.transitionProperty} ${s.transitionDuration} <- ${name(e)}${p}`);
    }
  }
  return [...out].sort();
});
console.log(sigs.join("\n"));
const n = Number(sample);
if (n > 0) {
  for (let t = 0; t < n; t += 250) {
    const v = await page.evaluate(() => {
      const dot = document.querySelector('[class*="cur"]');
      const edge = document.querySelector('[class*="edge"]');
      const r = dot?.getBoundingClientRect();
      return `${dot ? `${dot.getAttribute("class")} x=${r!.x.toFixed(0)} y=${r!.y.toFixed(0)}` : "no dot"} edge=${edge ? getComputedStyle(edge).opacity : "none"}`;
    });
    console.log(`t=${t} ${v}`);
    await page.waitForTimeout(250);
  }
}
await browser.close();
