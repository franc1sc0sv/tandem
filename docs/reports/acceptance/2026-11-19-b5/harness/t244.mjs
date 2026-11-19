// T-244: the other machine's "Allow remote execution" switch is locked: read-only, focusable,
// not dimmed, explains why, and neither a click nor Space/Enter sends anything to the agent or
// changes the tunnels module. Only "Keyboard and mouse" and "Clipboard" are real switches.
// Then the demo variants (win-win, mac-mac, admin=off) the task names.
// Usage: node t244.mjs <tag>  (needs the demo bridge on 38123 for the demo part)
import * as L from "./lib.mjs";
const tag = process.argv[2] ?? "fin"; const { m1, m2 } = L.attach(tag);
const F = "T-244-locked-switch";
const O = "http://127.0.0.1:38123";
const mod = async (m, id) => (await m.snapshot()).caps.here.modules.find((x) => x.id === id);
try {
  const me = (await m1.snapshot()).agent; const oid = me === "a" ? "b" : "a";
  const ctx = await (await L.getBrowser()).newContext({ viewport: { width: 1280, height: 860 }, locale: "en-US", colorScheme: "dark" });
  await ctx.addInitScript(() => { window.__calls = []; const of = window.fetch; window.fetch = async (u, o) => { if (/invoke/.test(String(u)) && !/snapshot|resources|_get|watch/.test(String(u))) window.__calls.push(String(u).replace("/__bridge/invoke/", "") + " " + String(o?.body ?? "").slice(0, 120)); return of(u, o); }; });
  await ctx.addInitScript(L.INIT);
  const p = await ctx.newPage(); p.errors = []; p.on("console", (m) => { if (m.type() === "error") p.errors.push(m.text()); }); p.on("pageerror", (e) => p.errors.push("pageerror: " + e.message));
  await p.goto(`${m1.origin}/#/machine/${oid}`); await L.sleep(2000);
  const sw = p.locator("button[role=switch][aria-label='Allow remote execution']");
  const info = await sw.evaluate((e) => ({ ro: e.getAttribute("aria-readonly"), ad: e.getAttribute("aria-disabled"), dis: e.disabled, title: e.title, desc: e.getAttribute("aria-describedby") ? document.getElementById(e.getAttribute("aria-describedby"))?.textContent : null, checked: e.getAttribute("aria-checked"), opacity: getComputedStyle(e).opacity, cursor: getComputedStyle(e).cursor, tab: e.tabIndex }));
  console.log("switch:", JSON.stringify(info));
  L.record(F, "the switch is read-only (aria-readonly), not natively disabled, not dimmed, focusable", info.ro === "true" && !info.dis && info.opacity === "1" && info.tab === 0 && info.cursor === "default", JSON.stringify(info));
  L.record(F, "the switch explains why (tooltip and accessible description)", /Always on: remote execution can't be turned off from here\./.test(info.title) && info.title === info.desc, JSON.stringify({ title: info.title, desc: info.desc }));
  const t0 = (await mod(m2, "tunnels")).state, t0here = (await mod(m1, "tunnels")).state;
  await p.evaluate(() => { window.__calls = []; });
  await sw.click({ force: true }); await L.sleep(800);
  await sw.focus(); const focused = await p.evaluate(() => document.activeElement?.getAttribute("aria-label"));
  await p.keyboard.press("Space"); await L.sleep(300); await p.keyboard.press("Enter"); await L.sleep(1500);
  const calls = await p.evaluate(() => window.__calls);
  const after = await sw.getAttribute("aria-checked");
  const t1 = (await mod(m2, "tunnels")).state, t1here = (await mod(m1, "tunnels")).state;
  console.log("calls after click/Space/Enter:", JSON.stringify(calls), "tunnels:", t0, "->", t1, "here:", t0here, "->", t1here);
  L.record(F, "click, Space and Enter have no effect: no command sent, aria-checked and the tunnels module unchanged on both agents", calls.length === 0 && after === info.checked && t0 === t1 && t0here === t1here && focused === "Allow remote execution", `calls=${JSON.stringify(calls)} checked ${info.checked}->${after} other ${t0}->${t1} here ${t0here}->${t1here} focused=${focused}`);
  await sw.hover(); await L.sleep(300);
  await L.shot(p, F, "01-other-machine-locked-switch");
  // Only "Keyboard and mouse" and "Clipboard" are switchable, on both pages.
  const lists = {};
  for (const id of [oid, me]) {
    await p.goto(`${m1.origin}/#/machine/${id}`); await L.sleep(1500);
    lists[id] = await p.locator("button[role=switch]").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") + (e.getAttribute("aria-readonly") ? " (locked)" : "")));
  }
  console.log("switches:", JSON.stringify(lists));
  const ok = (l) => l.filter((x) => !/locked/.test(x)).sort().join(",") === "Clipboard,Keyboard and mouse";
  L.record(F, "only Keyboard and mouse and Clipboard are real switches on both machine pages", ok(lists[oid]) && ok(lists[me]) && lists[me].every((x) => !/locked/.test(x)), JSON.stringify(lists));
  L.record(F, "no console errors on the live pages", p.errors.length === 0, p.errors.join(" ; ").slice(0, 300));
  await ctx.close();

  // Demo part.
  const demo = [];
  for (const [q, label] of [["?demo=win-win&notices", "win-win"], ["?demo=mac-mac&notices", "mac-mac"]]) {
    const pg = await L.openDemo(O, q, "#/desk"); await L.sleep(1500);
    const notices = (await pg.locator("[role=status], [role=alert], .toast, .notice").allInnerTexts().catch(() => [])).join(" / ");
    const desk = (await L.text(pg)).replace(/\n+/g, " | ");
    const sys = label === "win-win" ? /macOS|Mac\b|⌘|zsh|Homebrew/ : /Windows|PC\b|PowerShell|WSL|Ctrl\+(?!K)/; // Ctrl+K is the palette label of the browser this runs in (isMacPlatform), not of the demo pair;
    const m = (notices + " " + desk).match(sys);
    demo.push(`${label}: ${m ? "names " + m[0] : "ok"}`);
    L.record(F, `demo ${label} with notices names only the desk's machines and environments`, !m, m ? `«${m[0]}» in ${(notices + " " + desk).slice(Math.max(0, (notices + " " + desk).search(sys) - 80), (notices + " " + desk).search(sys) + 60)}` : notices.slice(0, 200));
    await L.shot(pg, F, `demo-${label}-notices`);
    for (const id of ["a", "b"]) {
      await pg.goto(`${O}/${q}#/machine/${id}`); await L.sleep(1200);
      const t = await L.text(pg);
      const sws = await pg.locator("button[role=switch]").evaluateAll((els) => els.map((e) => e.getAttribute("aria-label") + (e.getAttribute("aria-readonly") ? " (locked)" : "")));
      const perm = /Open System Settings/.test(t);
      L.record(F, `demo ${label} machine ${id}: only the two real switches; Permissions card only on macOS`, ok(sws) && (label === "win-win" ? !perm : true) && pg.errors.length === 0, JSON.stringify({ sws, perm, errors: pg.errors }));
    }
    await pg.ctx.close();
  }
  // The administrator note follows this machine's system: mac-win here=a is macOS, here=b is Windows.
  for (const [q, want] of [["?demo&wizard=0&admin=off", "macos"], ["?demo&here=b&wizard=0&admin=off", "windows"], ["?demo=win-win&wizard=0&admin=off", "windows"], ["?demo=mac-mac&wizard=0&admin=off", "macos"]]) {
    const pg = await L.openDemo(O, q, ""); await L.sleep(1800);
    const t = await L.text(pg);
    const macNote = /Move Tandem to Applications/.test(t), winNote = /Install Tandem in Program Files/.test(t);
    console.log("demo", q, { macNote, winNote });
    L.record(F, `demo ${q}: the administrator note is the ${want} one`, want === "macos" ? macNote && !winNote : winNote && !macNote, JSON.stringify({ macNote, winNote, errors: pg.errors }));
    await L.shot(pg, F, `demo-admin-off-${q.replace(/[?&=]/g, "_")}`);
    await pg.ctx.close();
  }
} catch (e) { console.log("ERROR", e); } finally { await L.closeBrowser(); }
