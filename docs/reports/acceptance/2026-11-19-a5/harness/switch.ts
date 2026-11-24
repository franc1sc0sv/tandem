// Acceptance part A, round 5: what the "Allow remote execution" switch (T-244, locked) does
// when the user clicks it or presses Space, in each pair and trouble state.
//
//   node docs/reports/acceptance/2026-11-19-a5/harness/switch.ts <out-dir>

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
  ["mac-win", "demo&still", "#/machine/b"],
  ["mac-win-here-b", "demo&here=b&still", "#/machine/a"],
  ["win-win", "demo=win-win&still", "#/machine/b"],
  ["mac-mac", "demo=mac-mac&still", "#/machine/b"],
  ["peer-off", "demo&still&trouble=peer-off", "#/machine/b"],
  ["link-down", "demo&still&trouble=link-down", "#/machine/b"],
  ["no-agent", "demo&still&trouble=no-agent", "#/machine/b"],
] as const;

const browser = await chromium.launch({ args: [...MR.CHROMIUM_ARGS] });
for (const [name, query, hash] of CASES) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 1, colorScheme: "dark" });
  const page = await ctx.newPage();
  const errors: string[] = [];
  page.on("console", (m) => (m.type() === "error" || m.type() === "warning") && errors.push(m.text()));
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
  const sw = page.locator('[role="switch"][aria-label="Allow remote execution"]');
  if ((await sw.count()) === 0) {
    console.log(`${name}: no "Allow remote execution" switch`);
    await ctx.close();
    continue;
  }
  const state = async () =>
    sw.evaluate((e) => {
      const s = getComputedStyle(e);
      const k = getComputedStyle(e, "::after");
      return { checked: e.getAttribute("aria-checked"), readonly: e.getAttribute("aria-readonly"), ariaDisabled: e.getAttribute("aria-disabled"), disabled: (e as HTMLButtonElement).disabled, title: e.getAttribute("title"), cursor: s.cursor, opacity: s.opacity, bg: s.backgroundColor, knob: k.transform };
    });
  const modules = await page.locator('section[aria-label="Modules"], section:has(h2:has-text("Modules"))').first().innerText().catch(() => "");
  const tunnelsRow = modules.split("\n").filter((l) => /Tunnels|Remote/i.test(l)).join(" / ");
  const before = await state();
  await sw.hover();
  await MR.settle(page);
  await page.screenshot({ path: join(OUT, `switch-${name}.png`), clip: await page.locator("section.envs").boundingBox().then((b) => b ?? { x: 0, y: 0, width: 1280, height: 860 }) });
  await sw.click({ force: true });
  await MR.settle(page);
  const afterClick = await state();
  await sw.focus();
  await page.keyboard.press("Space");
  await MR.settle(page);
  const afterKey = await state();
  const toasts = await page.locator('[role="status"], [role="alert"], .toast').allInnerTexts();
  console.log(JSON.stringify({ name, tunnelsRow, before, afterClick: afterClick.checked, afterKey: afterKey.checked, toasts: toasts.filter(Boolean), errors }));
  await ctx.close();
}
await browser.close();
