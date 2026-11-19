// Acceptance part A, round 4: visible-text, visual-rule, reduced-motion and animation sweep
// over the demo build (`apps/tandem-ui/dist-demo`, built by `pnpm shots`).
//
//   node docs/reports/acceptance/2026-11-19-a4/harness/sweep.ts <out-dir>
//
// Writes <out-dir>/sweep.json, <out-dir>/text.txt and screenshots of the new screens.
// Injected states (`inject=`) rewrite the demo bundle in the route handler: the Network notices
// use the exact texts of `crates/tandem-link/src/metrics/notices.rs`, and step 4 gets a hook to
// play a key heard on the other machine (what `AgentToUi::Notice(key_probe { second })` does live).

import { createRequire } from "node:module";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const REPO = fileURLToPath(new URL("../../../../../", import.meta.url));
const APP = join(REPO, "apps", "tandem-ui");
const DIST = join(APP, "dist-demo");
const req = createRequire(join(APP, "package.json"));
const { chromium } = req("playwright") as typeof import("playwright");
type Page = import("playwright").Page;
type Browser = import("playwright").Browser;
const MR = await import(join(REPO, "tools", "mockup-render", "src", "index.ts"));
const SPEC = await import(join(APP, "scripts", "shots-spec.ts"));

const OUT = process.argv[2] ?? "sweep-out";
mkdirSync(OUT, { recursive: true });
const ORIGIN = "http://app.local";
const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".woff2": "font/woff2", ".woff": "font/woff", ".svg": "image/svg+xml", ".png": "image/png" };
const MAC_UA = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
const WIN_UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

// The exact notices `link_notices` builds (T-223), one per kind plus the manager one.
const NOTICES = JSON.stringify([
  { interface: "Ethernet path", message: "169.254.252.182:47770 also reaches the Ethernet path; only [fe80::2%3]:47770 is used. Remove the extra peer from [network] in tandem.toml to silence this.", command: "" },
  { interface: "Link manager", message: "not running, so there is no link with the other machine: invalid configuration: the port cannot be 0", command: "" },
]);

interface Case {
  name: string;
  query: string;
  hash: string;
  page?: "index.html" | "tray.html";
  width?: number;
  height?: number;
  ua?: "mac" | "win";
  clicks?: string[];
  fills?: { selector: string; text: string }[];
  keys?: string[];
  inject?: "notices" | "probe";
  /** JS run after the clicks (for the injected hooks). */
  after?: string;
  shot?: string;
}

const ROUTES = ["#/desk", "#/tunnels", "#/monitors", "#/monitors/scene", "#/progress", "#/arrangement", "#/arrangement/crossing", "#/network", "#/machine/a", "#/machine/b", "#/settings", "#/states"];
const cases: Case[] = [];
for (const s of SPEC.SHOTS) cases.push({ name: `shot:${s.name}`, query: s.query, hash: s.hash, page: s.page, width: s.content?.appWidth ?? s.width, height: s.height, clicks: s.clicks, fills: s.fills, keys: s.keys });
for (const pair of ["mac-win", "win-win", "mac-mac"]) {
  for (const here of ["a", "b"]) for (const r of ROUTES) cases.push({ name: `${pair}:${here}:${r}`, query: `demo=${pair}&here=${here}&still`, hash: r, ua: pair === "win-win" ? "win" : pair === "mac-mac" ? "mac" : here === "a" ? "mac" : "win" });
  for (const t of ["no-agent", "peer-off", "link-down", "permissions"]) for (const r of ["#/desk", "#/tunnels", "#/monitors", "#/network", "#/machine/b"]) cases.push({ name: `${pair}:trouble-${t}:${r}`, query: `demo=${pair}&still&trouble=${t}`, hash: r });
  for (let w = 0; w <= 5; w++) cases.push({ name: `${pair}:wizard-${w}`, query: `demo=${pair}&still&wizard=${w}`, hash: "#/desk", ua: pair === "win-win" ? "win" : "mac" });
  cases.push({ name: `${pair}:setup-unpaired`, query: `demo=${pair}&still&setup&peer=none`, hash: "#/desk" });
  cases.push({ name: `${pair}:pair-code`, query: `demo=${pair}&still&wizard=1&peer=none&pair=code`, hash: "#/desk" });
  cases.push({ name: `${pair}:pair-reject`, query: `demo=${pair}&still&wizard=1&peer=none&pair=reject`, hash: "#/desk" });
  cases.push({ name: `${pair}:palette`, query: `demo=${pair}&still`, hash: "#/desk", keys: [pair === "win-win" ? "Control+k" : "Meta+k"], ua: pair === "win-win" ? "win" : "mac" });
  cases.push({ name: `${pair}:tray`, query: `demo=${pair}&still`, hash: "", page: "tray.html", width: 380, height: 600 });
  cases.push({ name: `${pair}:notices`, query: `demo=${pair}&still&notices`, hash: "#/desk" });
  cases.push({ name: `${pair}:peeredit`, query: `demo=${pair}&still&peeredit`, hash: "#/settings" });
}
// The screens develop gained since round 3.
const NEW: Case[] = [
  { name: "new:setup1-admin-pending", query: "demo&still&wizard=0&admin=pending", hash: "#/desk", shot: "setup1-admin-pending" },
  { name: "new:setup1-admin-pending-installed", query: "demo&still&wizard=0&admin=pending", hash: "#/desk", clicks: ['button:has-text("Install")'], shot: "setup1-admin-installed" },
  { name: "new:setup1-admin-off", query: "demo&still&wizard=0&admin=off", hash: "#/desk", shot: "setup1-admin-off" },
  { name: "new:setup1-admin-ok", query: "demo&still&wizard=0", hash: "#/desk", shot: "setup1-admin-ok" },
  { name: "new:setup1-win-win", query: "demo=win-win&still&wizard=0&admin=pending", hash: "#/desk", ua: "win", shot: "setup1-win-win-admin-pending" },
  { name: "new:machine-windows-protected", query: "demo&still", hash: "#/machine/b", shot: "machine-windows-protected-screens" },
  { name: "new:machine-windows-protected-win-win", query: "demo=win-win&still", hash: "#/machine/a", ua: "win", shot: "machine-win-win-protected-screens" },
  { name: "new:network-notices", query: "demo&still", hash: "#/network", inject: "notices", shot: "network-notices" },
  { name: "new:network-notices-win-win", query: "demo=win-win&still", hash: "#/network", inject: "notices", ua: "win", shot: "network-notices-win-win" },
  { name: "new:setup4-idle", query: "demo&still&wizard=3", hash: "#/desk", inject: "probe", shot: "setup4-idle" },
  { name: "new:setup4-here", query: "demo&still&wizard=3", hash: "#/desk", inject: "probe", keys: ["Shift"], shot: "setup4-activity-here" },
  { name: "new:setup4-both", query: "demo&still&wizard=3", hash: "#/desk", inject: "probe", keys: ["Shift"], after: "window.__heard('b')", shot: "setup4-activity-both" },
  { name: "new:setup4-other-only", query: "demo&still&wizard=3", hash: "#/desk", inject: "probe", after: "window.__heard('b')", shot: "setup4-activity-other" },
  { name: "new:setup5-lead", query: "demo&still&wizard=4", hash: "#/desk", shot: "setup5-lead" },
  { name: "new:crossing-crossed", query: "demo&still&cross=ok", hash: "#/arrangement/crossing", clicks: ['button:has-text("Test crossing")'], shot: "crossing-crossed" },
  { name: "new:settings-updates", query: "demo&still", hash: "#/settings", clicks: ['[role="tab"]:has-text("Updates")'], shot: "settings-updates" },
  { name: "new:progress-fail", query: "demo&still&txn=fail", hash: "#/progress", shot: "progress-rollback" },
  { name: "new:pair-code-match", query: "demo&still&wizard=1&peer=none&pair=code", hash: "#/desk", shot: "setup2-pair-code" },
];
cases.push(...NEW);

function patch(js: string, inject: Case["inject"]): string {
  if (inject === "notices") {
    const n = js.split("measuring:null,notices:[],now:").length - 1;
    if (n < 1) throw new Error("notices hook not found in the bundle");
    return js.replaceAll("measuring:null,notices:[],now:", `measuring:null,notices:${NOTICES},now:`);
  }
  if (inject === "probe") {
    const hook = "async watchKeyProbe(e){return this.listeners.add(e),";
    if (!js.includes(hook)) throw new Error("probe hook not found in the bundle");
    return js.replace(hook, "async watchKeyProbe(e){window.__heard=(m)=>this.heardKey(m);return this.listeners.add(e),");
  }
  return js;
}

async function serve(page: Page, errors: string[], inject: Case["inject"]): Promise<void> {
  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin !== ORIGIN) {
      errors.push(`external request: ${url.href}`);
      return route.abort("blockedbyclient");
    }
    const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "") || "index.html";
    const path = join(DIST, rel);
    if (!path.startsWith(DIST + sep) || !existsSync(path)) return route.fulfill({ status: 404, body: "not found" });
    let body: Buffer | string = readFileSync(path);
    if (inject && extname(path) === ".js" && rel.includes("source-")) body = patch(body.toString("utf8"), inject);
    return route.fulfill({ body, contentType: TYPES[extname(path)] ?? "application/octet-stream" });
  });
}

/** Everything a page shows and how it is drawn. */
function audit() {
  const vis = (e: Element) => {
    const r = e.getBoundingClientRect();
    const s = getComputedStyle(e);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0.05;
  };
  const text = document.body.innerText;
  const attrs: string[] = [];
  for (const e of Array.from(document.querySelectorAll("[aria-label],[title],[placeholder],img[alt]"))) {
    for (const a of ["aria-label", "title", "placeholder", "alt"]) {
      const v = e.getAttribute(a);
      if (v) attrs.push(v);
    }
  }
  // Buttons: icon, height, label.
  const buttons = Array.from(document.querySelectorAll("button, a.btn, [role=button]"))
    .filter(vis)
    .map((b) => {
      const r = b.getBoundingClientRect();
      return {
        label: (b.getAttribute("aria-label") || (b as HTMLElement).innerText || "").trim().slice(0, 60),
        cls: (b.getAttribute("class") || "").slice(0, 60),
        role: b.getAttribute("role") || "",
        icon: !!b.querySelector("svg, img") || getComputedStyle(b, "::before").content !== "none",
        h: Math.round(r.height),
        disabled: (b as HTMLButtonElement).disabled === true,
      };
    });
  // Cards with a colored line on top: a top border different from the rest, or a thin top pseudo element.
  const topLines: string[] = [];
  for (const e of Array.from(document.querySelectorAll("section, article, .card, [class*=card]"))) {
    if (!vis(e)) continue;
    const s = getComputedStyle(e);
    const tw = parseFloat(s.borderTopWidth);
    const sides = [s.borderRightWidth, s.borderBottomWidth, s.borderLeftWidth].map(parseFloat);
    if (tw >= 2 && sides.every((w) => w < tw)) topLines.push(`${e.tagName}.${e.className} border-top ${s.borderTopWidth} ${s.borderTopColor}`);
    else if (tw > 0 && s.borderTopColor !== s.borderLeftColor && sides.some((w) => w > 0)) topLines.push(`${e.tagName}.${e.className} top color ${s.borderTopColor} vs ${s.borderLeftColor}`);
    for (const p of ["::before", "::after"]) {
      const ps = getComputedStyle(e, p);
      if (ps.content === "none" || ps.position !== "absolute") continue;
      const h = parseFloat(ps.height);
      if (h > 0 && h <= 4 && (ps.top === "0px" || ps.top === "-1px") && ps.backgroundColor !== "rgba(0, 0, 0, 0)" && parseFloat(ps.width) > 40) topLines.push(`${e.tagName}.${e.className}${p} h=${ps.height} bg=${ps.backgroundColor}`);
    }
  }
  // Chips: height and radius per class.
  const chips = Array.from(document.querySelectorAll(".chip, [class*=chip], .st"))
    .filter(vis)
    .map((c) => {
      const s = getComputedStyle(c);
      return `${(c.getAttribute("class") || "").split(" ")[0]}|h=${Math.round(c.getBoundingClientRect().height)}|r=${s.borderRadius}|fs=${s.fontSize}`;
    });
  // Declared motion: running animations, and elements with a transition or an animation.
  const anims = document.getAnimations().filter((a) => a.playState === "running").length;
  let withAnim = 0;
  let withTrans = 0;
  for (const e of Array.from(document.querySelectorAll("*"))) {
    const s = getComputedStyle(e);
    if (s.animationName !== "none" && s.animationDuration.split(",").some((d) => parseFloat(d) > 0)) withAnim++;
    if (s.transitionDuration.split(",").some((d) => parseFloat(d) > 0) && s.transitionProperty !== "none") withTrans++;
    for (const p of ["::before", "::after"]) {
      const ps = getComputedStyle(e, p);
      if (ps.content !== "none" && ps.animationName !== "none" && ps.animationDuration.split(",").some((d) => parseFloat(d) > 0)) withAnim++;
    }
  }
  const sidebar = Array.from(document.querySelectorAll('nav[aria-label="Main"] a, nav[aria-label="Main"] button')).map((a) => (a as HTMLElement).innerText.trim().replace(/\s+/g, " "));
  return { text, attrs, buttons, topLines, chips, anims, withAnim, withTrans, sidebar };
}

// The same accents and word list as the repository guard (scripts/ci/english.sh), read from it.
const GUARD = readFileSync(join(REPO, "scripts", "ci", "english.sh"), "utf8");
const ACCENTS = /^ACCENTS='([^']*)'/m.exec(GUARD)?.[1] ?? "";
const WORDS = /^WORDS='([^']*)'/m.exec(GUARD)?.[1] ?? "";
if (!ACCENTS || !WORDS) throw new Error("could not read the word list of scripts/ci/english.sh");
const SPANISH = new RegExp(`${ACCENTS}|(?<![A-Za-z0-9.])(?:${WORDS})(?![A-Za-z0-9])`, "i");
const BAD = ["{{", "undefined", "NaN", "[object", " null"];
const FORBIDDEN = [/keyboard here/i, /add (a )?machine/i, /requires? (a )?cable/i, /third machine/i, /\bPC → PC\b/, /\bMac → Mac\b/, /\bmode\b/i];
const NOT_ON_WIN = [/\bmacOS\b/, /\bMac\b/, /MacBook/, /\bzsh\b/, /LaunchDaemon/, /⌘/];
const NOT_ON_MAC = [/\bWindows\b/, /\bPC\b/, /\bWSL\b/, /PowerShell/, /\bUAC\b/, /\bPC-Desktop\b/];

interface Result {
  name: string;
  errors: string[];
  findings: string[];
  buttonsNoIcon: string[];
  heights: Record<string, number>;
  topLines: string[];
  chips: string[];
  reduced: { anims: number; withAnim: number; withTrans: number };
  normal: { anims: number; withAnim: number; withTrans: number };
  sidebar: string[];
}

async function run(browser: Browser, c: Case, reduced: boolean): Promise<{ a: ReturnType<typeof audit>; errors: string[] }> {
  const errors: string[] = [];
  const ctx = await browser.newContext({ viewport: { width: c.width ?? 1280, height: c.height ?? 860 }, deviceScaleFactor: 1, locale: "en-US", timezoneId: "UTC", colorScheme: "dark", reducedMotion: reduced ? "reduce" : "no-preference", userAgent: c.ua === "win" ? WIN_UA : MAC_UA });
  try {
    const page = await ctx.newPage();
    page.on("console", (m) => {
      if (m.type() === "error" || m.type() === "warning") errors.push(`console ${m.type()}: ${m.text()}`);
    });
    page.on("pageerror", (e) => errors.push(`exception: ${e.message}`));
    await serve(page, errors, c.inject);
    await page.goto(`${ORIGIN}/${c.page ?? "index.html"}${c.query ? `?${c.query}` : ""}${c.hash}`);
    await page.waitForSelector("#app > *", { timeout: 10_000 });
    await MR.settle(page);
    for (const f of c.fills ?? []) {
      await page.locator(f.selector).first().fill(f.text, { timeout: 5_000 });
      await MR.settle(page);
    }
    for (const sel of c.clicks ?? []) {
      await page.locator(sel).first().click({ timeout: 5_000 });
      await MR.settle(page);
    }
    for (const k of c.keys ?? []) {
      await page.keyboard.press(k);
      await MR.settle(page);
    }
    if (c.after) {
      await page.evaluate(c.after);
      await MR.settle(page);
    }
    await page.mouse.move(0, 0);
    await MR.settle(page);
    // Infinite animations never settle: the reduced-motion count is taken after the entrance window.
    await page.waitForTimeout(reduced ? 300 : 50);
    const a = await page.evaluate(audit);
    if (c.shot && !reduced) await page.screenshot({ path: join(OUT, `${c.shot}.png`), animations: "disabled" });
    return { a, errors };
  } catch (e) {
    errors.push(`harness: ${(e as Error).message.split("\n")[0]}`);
    return { a: { text: "", attrs: [], buttons: [], topLines: [], chips: [], anims: 0, withAnim: 0, withTrans: 0, sidebar: [] }, errors };
  } finally {
    await ctx.close();
  }
}

const browser = await chromium.launch({ args: [...MR.CHROMIUM_ARGS] });
const results: Result[] = [];
const lines = new Set<string>();
const textDump: string[] = [];
try {
  for (const c of cases) {
    const n = await run(browser, c, false);
    const r = await run(browser, c, true);
    const findings: string[] = [];
    const all = [n.a.text, ...n.a.attrs].join("\n");
    for (const l of all.split("\n").map((x) => x.trim()).filter(Boolean)) {
      lines.add(l);
      if (SPANISH.test(l)) findings.push(`spanish? ${l}`);
      for (const b of BAD) if (l.includes(b)) findings.push(`bad token ${b}: ${l}`);
      for (const f of FORBIDDEN) if (f.test(l)) findings.push(`forbidden ${f}: ${l}`);
      if (c.name.startsWith("win-win")) for (const f of NOT_ON_WIN) if (f.test(l)) findings.push(`mac on win-win ${f}: ${l}`);
      if (c.name.startsWith("mac-mac")) for (const f of NOT_ON_MAC) if (f.test(l)) findings.push(`pc on mac-mac ${f}: ${l}`);
    }
    if (c.hash.startsWith("#/arrangement") && /\b[XY]\s*[:=]\s*-?\d/.test(n.a.text)) findings.push("X/Y coordinates in the arrangement");
    const heights: Record<string, number> = {};
    for (const b of n.a.buttons) heights[`${b.h}`] = (heights[`${b.h}`] ?? 0) + 1;
    textDump.push(`== ${c.name}\n${n.a.text}\n-- attrs: ${n.a.attrs.join(" | ")}`);
    results.push({
      name: c.name,
      errors: [...new Set([...n.errors, ...r.errors])],
      findings: [...new Set(findings)],
      buttonsNoIcon: [...new Set(n.a.buttons.filter((b) => !b.icon).map((b) => `${b.label} [${b.cls}|${b.role}|h${b.h}]`))],
      heights,
      topLines: n.a.topLines,
      chips: [...new Set(n.a.chips)],
      reduced: { anims: r.a.anims, withAnim: r.a.withAnim, withTrans: r.a.withTrans },
      normal: { anims: n.a.anims, withAnim: n.a.withAnim, withTrans: n.a.withTrans },
      sidebar: n.a.sidebar,
    });
    process.stdout.write(`${c.name} errors=${results.at(-1)!.errors.length} findings=${findings.length} reduced=${r.a.anims}/${r.a.withAnim}/${r.a.withTrans}\n`);
  }
} finally {
  await browser.close();
}
writeFileSync(join(OUT, "sweep.json"), JSON.stringify(results, null, 1));
writeFileSync(join(OUT, "text.txt"), textDump.join("\n\n"));
writeFileSync(join(OUT, "lines.txt"), [...lines].sort().join("\n"));
const tot = (f: (r: Result) => number) => results.reduce((s, r) => s + f(r), 0);
console.log(`cases=${results.length} lines=${lines.size} errors=${tot((r) => r.errors.length)} findings=${tot((r) => r.findings.length)} reducedAnims=${tot((r) => r.reduced.anims)} reducedDecl=${tot((r) => r.reduced.withAnim + r.reduced.withTrans)} topLines=${tot((r) => r.topLines.length)}`);
