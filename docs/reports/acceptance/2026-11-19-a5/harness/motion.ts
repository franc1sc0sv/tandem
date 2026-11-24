// Acceptance part A, round 5: declared animations and transitions of each mockup state against
// the app after the same clicks (every `pnpm shots` case with a mockup).
//
//   node docs/reports/acceptance/2026-11-19-a5/harness/motion.ts <out-dir>
//
// Signature: `anim|<ms>|<iterations>|<animated properties>` and `trans|<property>|<ms>`. A mockup
// signature is "matched" if the app has the same kind, properties and iterations; the
// duration difference is reported apart.

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
const MR = await import(join(REPO, "tools", "mockup-render", "src", "index.ts"));
const SPEC = await import(join(APP, "scripts", "shots-spec.ts"));
const OUT = process.argv[2] ?? "motion-out";
mkdirSync(OUT, { recursive: true });
const ORIGIN = "http://app.local";
const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css; charset=utf-8", ".woff2": "font/woff2", ".woff": "font/woff", ".svg": "image/svg+xml" };

function signatures(): string[] {
  const frames = new Map<string, string>();
  const walk = (rules: CSSRuleList) => {
    for (const r of Array.from(rules)) {
      if (r instanceof CSSKeyframesRule) {
        const props = new Set<string>();
        for (const k of Array.from(r.cssRules) as CSSKeyframeRule[]) for (let i = 0; i < k.style.length; i++) props.add(k.style[i]!.replace(/^(margin|padding|inset)-.*/, "$1"));
        frames.set(r.name, [...props].sort().join(","));
      } else if ("cssRules" in r && (r as CSSGroupingRule).cssRules) walk((r as CSSGroupingRule).cssRules);
    }
  };
  for (const s of Array.from(document.styleSheets)) {
    try {
      walk(s.cssRules);
    } catch {
      /* cross-origin */
    }
  }
  const ms = (d: string) => Math.round(parseFloat(d) * (d.endsWith("ms") ? 1 : 1000));
  const out = new Set<string>();
  const one = (s: CSSStyleDeclaration) => {
    if (s.animationName !== "none") {
      const names = s.animationName.split(",").map((x) => x.trim());
      const durs = s.animationDuration.split(",");
      const its = s.animationIterationCount.split(",");
      names.forEach((n, i) => {
        const d = ms(durs[i % durs.length]!.trim());
        if (d > 0) out.add(`anim|${d}|${its[i % its.length]!.trim() === "infinite" ? "inf" : its[i % its.length]!.trim()}|${frames.get(n) ?? `?${n}`}`);
      });
    }
    if (s.transitionProperty !== "none") {
      const props = s.transitionProperty.split(",").map((x) => x.trim());
      const durs = s.transitionDuration.split(",");
      props.forEach((p, i) => {
        const d = ms(durs[i % durs.length]!.trim());
        if (d > 0) out.add(`trans|${p}|${d}`);
      });
    }
  };
  for (const e of Array.from(document.querySelectorAll("body *"))) {
    one(getComputedStyle(e));
    for (const p of ["::before", "::after"]) {
      const ps = getComputedStyle(e, p);
      if (ps.content !== "none") one(ps);
    }
  }
  return [...out].sort();
}

const browser = await chromium.launch({ args: [...MR.CHROMIUM_ARGS] });
const report: string[] = [];
const summary: { shot: string; mockup: string; missing: string[]; duration: string[] }[] = [];
try {
  for (const s of SPEC.SHOTS) {
    if (s.page === "tray.html" && s.mockup !== "V3-Tray") continue;
    // Mockup.
    const mctx = await browser.newContext({ viewport: { width: s.width, height: s.height }, deviceScaleFactor: 1, colorScheme: "dark" });
    await MR.installRoutes(mctx);
    const mp = await mctx.newPage();
    await mp.goto(`${MR.ORIGIN}/${s.mockup}.dc.html`);
    await MR.waitForMockup(mp);
    const mock = new Set<string>(await mp.evaluate(signatures));
    const st = s.state ? MR.statesOf(s.mockup).find((x: { name: string }) => x.name === s.state) : undefined;
    for (const step of st?.steps ?? []) {
      if ("click" in step) await mp.locator(step.click).click({ timeout: 5_000 }).catch(() => undefined);
      else await mp.locator(step.fill).fill(step.text, { timeout: 5_000 }).catch(() => undefined);
      await mp.waitForTimeout(100);
      for (const x of await mp.evaluate(signatures)) mock.add(x);
    }
    await mctx.close();
    // App.
    const actx = await browser.newContext({ viewport: { width: s.content?.appWidth ?? s.width, height: s.height }, deviceScaleFactor: 1, colorScheme: "dark", userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36" });
    const ap: Page = await actx.newPage();
    await ap.route("**/*", async (route) => {
      const url = new URL(route.request().url());
      if (url.origin !== ORIGIN) return route.abort("blockedbyclient");
      const rel = normalize(decodeURIComponent(url.pathname)).replace(/^([/\\])+/, "") || "index.html";
      const path = join(DIST, rel);
      if (!path.startsWith(DIST + sep) || !existsSync(path)) return route.fulfill({ status: 404, body: "" });
      return route.fulfill({ body: readFileSync(path), contentType: TYPES[extname(path)] ?? "application/octet-stream" });
    });
    await ap.goto(SPEC.appUrl(ORIGIN, s));
    await ap.waitForSelector("#app > *", { timeout: 10_000 });
    const app = new Set<string>(await ap.evaluate(signatures));
    for (const f of s.fills ?? []) await ap.locator(f.selector).first().fill(f.text).catch(() => undefined);
    for (const c of s.clicks ?? []) {
      await ap.locator(c).first().click({ timeout: 5_000 }).catch(() => undefined);
      await ap.waitForTimeout(100);
      for (const x of await ap.evaluate(signatures)) app.add(x);
    }
    for (const k of s.keys ?? []) {
      await ap.keyboard.press(k);
      await ap.waitForTimeout(100);
      for (const x of await ap.evaluate(signatures)) app.add(x);
    }
    await actx.close();
    const key = (x: string) => {
      const p = x.split("|");
      return p[0] === "anim" ? `anim|${p[2]}|${p[3]}` : `trans|${p[1]}`;
    };
    const appKeys = new Map<string, number[]>();
    for (const x of app) {
      const p = x.split("|");
      const d = Number(p[0] === "anim" ? p[1] : p[2]);
      appKeys.set(key(x), [...(appKeys.get(key(x)) ?? []), d]);
      // `all` covers any single property.
    }
    const missing: string[] = [];
    const duration: string[] = [];
    for (const x of mock) {
      const p = x.split("|");
      const d = Number(p[0] === "anim" ? p[1] : p[2]);
      const ds = appKeys.get(key(x)) ?? (p[0] === "trans" ? appKeys.get("trans|all") : undefined);
      if (!ds) missing.push(x);
      else if (!ds.some((v) => Math.abs(v - d) <= Math.max(60, d * 0.25))) duration.push(`${x} (app ${[...new Set(ds)].join("/")} ms)`);
    }
    summary.push({ shot: s.name, mockup: s.state ? `${s.mockup}--${s.state}` : s.mockup, missing, duration });
    report.push(`== ${s.name} (${s.mockup}${s.state ? `--${s.state}` : ""}): mockup ${mock.size}, app ${app.size}, missing ${missing.length}, duration ${duration.length}`);
    for (const m of missing) report.push(`  MISSING ${m}`);
    for (const m of duration) report.push(`  DURATION ${m}`);
    process.stdout.write(`${report.filter((l) => l.startsWith("==")).at(-1)}\n`);
  }
} finally {
  await browser.close();
}
writeFileSync(join(OUT, "motion.txt"), report.join("\n"));
writeFileSync(join(OUT, "motion.json"), JSON.stringify(summary, null, 1));
