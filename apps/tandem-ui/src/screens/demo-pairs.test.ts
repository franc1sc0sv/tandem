// T-244: the demo behaves like the real app for each pair of systems (D2): no Mac-only or
// PC-only texts and controls where the desk does not have that system.
import { render, screen, within } from "@testing-library/svelte";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../lib/context";
import { demoNotices } from "../lib/notices";
import { stateCards } from "../lib/states";
import { DemoSource, demoDesk, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Machine from "./Machine.svelte";
import States from "./States.svelte";

const MAC_ONLY = /\b(Mac|macOS|MacBook)\b/;
const WINDOWS_ONLY = /\b(PC|Windows|WSL|Program Files|UAC)\b|PC-/;

/** What each pair must not mention: the other family's names and system words. */
const PAIRS = [
  { pair: "mac-win", forbidden: null },
  { pair: "win-win", forbidden: MAC_ONLY },
  { pair: "mac-mac", forbidden: WINDOWS_ONLY },
] as const;

describe.each(PAIRS)("seeded notices in the $pair pair", ({ pair, forbidden }) => {
  for (const here of ["a", "b"] as const) {
    it(`window ${here}: they name only the desk's machines and environments`, () => {
      const d = demoDesk(demoOptions(`demo=${pair}&still&here=${here}`));
      const all = demoNotices(d).flatMap((n) => [n.text, n.body ?? ""]);
      expect(all).toHaveLength(6);
      if (forbidden) for (const t of all) expect(t).not.toMatch(forbidden);
      const names = [d.machines.a?.name, d.machines.b?.name];
      const tunnel = demoNotices(d)[2];
      expect(tunnel.body).toContain(`${d.machines[here]?.name} → ${d.machines[here === "a" ? "b" : "a"]?.name}`);
      const scene = demoNotices(d)[1];
      expect(names.some((n) => n && scene.body?.includes(n))).toBe(true);
    });
  }
});

describe("seeded notices in the mockup's pair", () => {
  it("read exactly as F-Notices", () => {
    const n = demoNotices(demoDesk(demoOptions("demo&still")));
    expect(n.map((x) => [x.text, x.body])).toEqual([
      ["The USB-C cable was unplugged", "Keyboard and mouse carry on over Wi-Fi. Data transfers wait for the cable."],
      ["Scene “Work” applied", '34" confirmed on PC-Desktop in 1.8 s.'],
      ["Claude opened the tunnel “tandem”", "MacBook Pro → PC-Desktop · WSL."],
    ]);
  });

  it("the environment is the peer's own: a Mac mini has no WSL", () => {
    const n = demoNotices(demoDesk(demoOptions("demo=mac-mac&still")));
    expect(n[2].body).toBe("MacBook Pro → Mac mini · zsh.");
  });

  it("the store seeds them held, newest on top", async () => {
    const store = new Store(new DemoSource(demoOptions("demo=win-win&still")));
    await store.start();
    store.seedDemoNotices();
    expect(store.toasts.map((t) => t.text)).toEqual(demoNotices(store.desk).map((n) => n.text));
    expect(store.toasts.every((t) => !t.auto)).toBe(true);
  });
});

describe.each(PAIRS)("trouble states reference view in the $pair pair", ({ pair }) => {
  for (const here of ["a", "b"] as const) {
    it(`window ${here}: the Permissions card exists only where the system asks for permissions`, async () => {
      const q = `demo=${pair}&still&here=${here}`;
      const d = demoDesk(demoOptions(q));
      const mac = d.machines[here]?.os === "macos";
      const cards = stateCards(d, { gallery: true, failedScene: null });
      expect(cards.some((c) => c.id === "permission")).toBe(mac);

      const store = new Store(new DemoSource(demoOptions(q)));
      await store.start();
      store.go({ screen: "states" });
      render(States, { context: new Map([[STORE_KEY, store]]) });
      await tick();
      expect(!!screen.queryByRole("button", { name: /Open System Settings/ })).toBe(mac);
      if (!mac) expect(document.body.textContent).not.toMatch(/System Settings|macOS|Input Monitoring/);
    });
  }

  it("a missing permission on a machine that has none does not make a card", () => {
    const d = demoDesk(demoOptions(`demo=${pair}&still&trouble=permissions&here=b`));
    const mac = d.machines.b?.os === "macos";
    expect(stateCards(d).some((c) => c.id === "permission")).toBe(mac);
  });
});

describe.each(PAIRS)("module switches in the $pair pair", ({ pair }) => {
  for (const machine of ["a", "b"] as const) {
    it(`the page of machine ${machine} switches only Keyboard and mouse and Clipboard`, async () => {
      const store = new Store(new DemoSource(demoOptions(`demo=${pair}&still`)));
      await store.start();
      store.go({ screen: "machine", machine });
      render(Machine, { props: { machine }, context: new Map([[STORE_KEY, store]]) });
      await tick();
      const mods = within(screen.getByRole("region", { name: "Modules" }));
      expect(mods.getAllByRole("switch").map((s) => s.getAttribute("aria-label") ?? s.textContent?.replace(/\s*(on|off)\s*$/, "").trim()).sort()).toEqual([
        "Clipboard",
        "Keyboard and mouse",
      ]);
      for (const name of ["Monitors", "Tunnels", "Remote compute", "Protected screens", "Power"]) expect(mods.queryByRole("switch", { name })).toBeNull();
      // The mockup's "Allow remote execution" block stays on the other machine's page, shown on and locked:
      // the agent cannot turn Tunnels off (DECISIONS T-098).
      const remote = screen.queryByRole("switch", { name: "Allow remote execution" });
      if (machine === store.desk.here) expect(remote).toBeNull();
      else {
        expect(remote).toBeInTheDocument();
        expect(remote).toHaveAttribute("aria-readonly", "true");
        expect(remote).toHaveAttribute("aria-checked", "true");
      }
    });
  }
});
