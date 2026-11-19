// Acceptance part A, round 6: T-270 removed the "Allow remote execution" row. On each machine
// page, in each pair and trouble state: the row is gone, the Tunnels module has no switch, no
// switch is read-only, and every switch that is left really toggles (click, then Space back).
//
//   node docs/reports/acceptance/2026-11-19-a6/harness/switch.ts <out-dir>

import { createRequire } from "node:module";
import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("../../../../../", import.meta.url));
const APP = join(REPO, "apps", "tandem-ui");
const DIST = join(APP, "dist-demo");
const req = createRequire(join(APP, "package.json"));
const { chromium } = req("playwright") as typeof import("playwright");
const MR = await import(join(REPO, "tools", "mockup-render", "src", "index.ts"));
const OUT = process.argv[2] ?? "switch-out";
mkdirSync(OUT, { recursive: true });
const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".woff2": "font/woff2", ".woff": "font/woff", ".svg": "image/svg+xml" };

const CASES = [
  ["mac-win-b", "demo&still", "#/machine/b"],
  ["mac-win-a", "demo&still", "#/machine/a"],
  ["mac-win-here-b-a", "demo&here=b&still", "#/machine/a"],
  ["mac-win-here-b-b", "demo&here=b&still", "#/machine/b"],
  ["win-win-a", "demo=win-win&still", "#/machine/a"],
  ["win-win-b", "demo=win-win&still", "#/machine/b"],
  ["mac-mac-a", "demo=mac-mac&still", "#/machine/a"],
  ["mac-mac-b", "demo=mac-mac&still", "#/machine/b"],
  ["peer-off-b", "demo&still&trouble=peer-off", "#/machine/b"],
  ["link-down-b", "demo&still&trouble=link-down", "#/machine/b"],
  ["no-agent-b", "demo&still&trouble=no-agent", "#/machine/b"],
] as const;

const browser = await chromium.launch({ args: [...MR.CHROMIUM_ARGS] });
let bad = 0;
for (const [name, query, hash] of CASES) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 1, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors: string[] = [];
  page.on("console", (m) => (m.type() === "error" || m.type() === "warning") && errors.push(m.text()));
  page.on("pageerror", (e) => errors.push(e.message));
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
  await MR.settle(page);
  const text = await page.locator("main").innerText();
  const remote = /remote execution/i.test(text) || (await page.locator('[aria-label*="remote execution" i]').count()) > 0;
  const tunnelsSwitch = await page.locator('[role="switch"][aria-label*="Tunnels"]').count();
  const envs = page.locator("section.envs");
  if ((await envs.count()) > 0) await envs.screenshot({ path: join(OUT, `envs-${name}.png`) });
  const sws = page.locator('[role="switch"]');
  const toggles: string[] = [];
  for (let i = 0; i < (await sws.count()); i++) {
    const sw = sws.nth(i);
    const label = await sw.getAttribute("aria-label");
    const ro = await sw.getAttribute("aria-readonly");
    const dis = await sw.isDisabled();
    const before = await sw.getAttribute("aria-checked");
    if (dis) {
      toggles.push(`${label}: disabled (${before})`);
      continue;
    }
    await sw.click();
    await MR.settle(page);
    const afterClick = await sw.getAttribute("aria-checked");
    await sw.focus();
    await page.keyboard.press("Space");
    await MR.settle(page);
    const afterKey = await sw.getAttribute("aria-checked");
    const ok = afterClick !== before && afterKey === before && ro === null;
    if (!ok) bad++;
    toggles.push(`${label}: ${before} -> click ${afterClick} -> Space ${afterKey} readonly=${ro ?? "-"} ${ok ? "ok" : "FAIL"}`);
  }
  if (remote || tunnelsSwitch > 0 || errors.length) bad++;
  console.log(JSON.stringify({ name, remoteExecution: remote, tunnelsSwitch, switches: toggles, errors }));
  await ctx.close();
}
await browser.close();
console.log(bad === 0 ? "switch: no findings" : `switch: ${bad} findings`);
