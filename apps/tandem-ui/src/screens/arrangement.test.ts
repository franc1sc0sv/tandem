import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { formatHash, parseHash } from "../lib/routes";
import { STORE_KEY } from "../lib/context";
import type { Action, Notice, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource, layoutFromDesk } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import Shell from "../shell/Shell.svelte";

async function open(source: Source, hash = "#/arrangement") {
  const store = new Store(source, { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const pad = () => screen.getByRole("radiogroup", { name: /Where is/ });
const align = () => screen.getByRole("radiogroup", { name: "Alignment" });
const slot = (name: string) => within(pad()).getByRole("radio", { name });
const alignTo = (name: string) => within(align()).getByRole("radio", { name });

describe("Arrangement (V3-Arrangement) in demo mode", () => {
  it("opens with the 34\" chosen, the other machine on the left and aligned at the bottom", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    expect(screen.getByRole("button", { name: /Odyssey G5 34"/ })).toHaveAttribute("aria-pressed", "true");
    expect(pad()).toHaveAccessibleName("Where is MacBook Pro?");
    expect(slot("Left")).toHaveAttribute("aria-checked", "true");
    const opts = [...align().querySelectorAll("[role=radio]")].map((b) => b.textContent);
    expect(opts).toEqual(["Top", "Center", "Bottom"]);
    expect(alignTo("Bottom")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("Crossing edge")).toBeInTheDocument();
    expect(screen.getByText("Work")).toBeInTheDocument();
    // Without X/Y (D18).
    expect(screen.queryByRole("spinbutton")).toBeNull();
    expect(screen.queryByText(/Advanced/)).toBeNull();
  });

  it("moving the other machine up changes the alignment labels and the edge becomes horizontal", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    const edge = () => document.querySelector<HTMLElement>(".xedge");
    expect(edge()?.style.width).toBe("3px");
    await userEvent.click(slot("Top"));
    expect(slot("Top")).toHaveAttribute("aria-checked", "true");
    expect(slot("Left")).toHaveAttribute("aria-checked", "false");
    const opts = [...align().querySelectorAll("[role=radio]")].map((b) => b.textContent);
    expect(opts).toEqual(["Left", "Center", "Right"]);
    expect(edge()?.style.height).toBe("3px");
  });

  it("saving sends the arrangement to the desk and announces it", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = await open(src);
    await userEvent.click(slot("Bottom"));
    await userEvent.click(alignTo("Center"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(store.desk.layout.arrangement).toEqual({ aSide: "down", align: "center" });
    expect(await screen.findByText("Arrangement saved")).toBeInTheDocument();
  });

  it("with the MacBook chosen the mini plan is seen from it", async () => {
    const store = await open(new DemoSource(demoOptions("demo&still")));
    await userEvent.click(screen.getByRole("button", { name: /MacBook Pro/ }));
    expect(pad()).toHaveAccessibleName('Where is the 34"?');
    expect(slot("Right")).toHaveAttribute("aria-checked", "true");
    // Putting the 34" above the MacBook is having the MacBook below the 34".
    await userEvent.click(slot("Top"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(store.desk.layout.arrangement.aSide).toBe("down");
  });

  it("a disconnected screen says it does not take part in the arrangement", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    await userEvent.click(screen.getByRole("button", { name: /Odyssey G5 32"/ }));
    expect(screen.getByText("Unsupported: it does not take part in the arrangement.")).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup", { name: "Alignment" })).toBeNull();
    expect(screen.getByText("disconnected")).toBeInTheDocument();
  });

  it("the mockup animations are there and are CSS (prefers-reduced-motion turns them off)", () => {
    const src = readFileSync(join(process.cwd(), "src", "screens", "Arrangement.svelte"), "utf8");
    for (const k of ["@keyframes gl ", "@keyframes gl2", "@keyframes fd", "cubic-bezier(0.3, 1.2, 0.5, 1)", "cubic-bezier(0.3, 1.3, 0.5, 1)", "cubic-bezier(0.3, 1.5, 0.5, 1)"]) expect(src).toContain(k);
    const cross = readFileSync(join(process.cwd(), "src", "screens", "CrossTest.svelte"), "utf8");
    for (const k of ["@keyframes eg", "@keyframes nud", "@keyframes cnt", "@keyframes bs", "@keyframes in"]) expect(cross).toContain(k);
  });
});

describe("Crossing test (F-CrossingTest)", () => {
  it("it hangs off the arrangement: `#/arrangement/crossing`", () => {
    expect(parseHash("#/arrangement/crossing")).toEqual({ screen: "crossing" });
    expect(formatHash({ screen: "crossing" })).toBe("#/arrangement/crossing");
    expect(parseHash("#/arrangement/other")).toEqual({ screen: "arrangement" });
  });


  it("a crossing without a measured position or latency does not invent them", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await open(src, "#/arrangement/crossing");
    src.cross({ from: "a", to: "b", edge: "right", at: 0, of: 0, ms: null });
    expect(await screen.findByText("Crossed fine")).toBeInTheDocument();
    expect(screen.getByText(/It entered PC-Desktop\./)).toBeInTheDocument();
    expect(screen.queryByText(/Latency/)).toBeNull();
  });

  it("asks to cross through the arrangement edge and passes with the crossing", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await open(src, "#/arrangement/crossing");
    expect(screen.getByText("Move the mouse to the right of the MacBook")).toBeInTheDocument();
    expect(screen.getByText("waiting for the cursor")).toBeInTheDocument();
    src.cross({ from: "a", to: "b", edge: "right", at: 612, of: 1440, ms: 0.4 });
    expect(await screen.findByText("Crossed fine")).toBeInTheDocument();
    expect(screen.getByText(/It entered PC-Desktop at the right height \(y = 612 of 1440\)\. Crossing latency: 0\.4 ms\./)).toBeInTheDocument();
    expect(screen.getByText("the cursor arrived here")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Test again/ }));
    expect(await screen.findByText("Move the mouse to the right of the MacBook")).toBeInTheDocument();
  });

  it("a crossing through another edge says which one and offers to edit the arrangement", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await open(src, "#/arrangement/crossing");
    src.cross({ from: "a", to: "b", edge: "up", at: 10, of: 3440, ms: 1 });
    expect(await screen.findByText("Crossed at another edge")).toBeInTheDocument();
    expect(screen.getByText(/Crossed at the top, not the right/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Edit arrangement/ })).toHaveAttribute("href", "#/arrangement");
  });

  it("without a crossing in time it times out", async () => {
    await open(new DemoSource(demoOptions("demo&still&cross=timeout")), "#/arrangement/crossing");
    expect(await screen.findByText("Did not cross in 20 s")).toBeInTheDocument();
    expect(screen.getByText(/Maybe the 34" is on another side of your desk/)).toBeInTheDocument();
  });

  it("the requested edge follows the saved arrangement", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await src.setArrangement({ aSide: "down", align: "start" });
    await open(src, "#/arrangement/crossing");
    // A is below B: from A you leave through the top.
    expect(screen.getByText("Move the mouse above the MacBook")).toBeInTheDocument();
  });
});

describe("with the real agent", () => {
  const connected = (): Snapshot => ({
    kind: "connected",
    agent: "a",
    desk: {
      machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } },
      screens: [
        { machine: "a", id: "Built-in", x: 0, y: 0, width: 1512, height: 945, scale: 2 },
        { machine: "b", id: "Odyssey", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
      ],
      arrangement: { a_side: "right", align: "start" },
      scenes: [],
    },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
      },
      peer: null,
    },
    monitors: [],
  });

  /** What the test does not use is left waiting. */
  function bridge(act: (a: Action) => Promise<unknown>): Bridge {
    const known: Partial<Record<keyof Bridge, unknown>> = { snapshot: async () => connected(), onSnapshot: async () => {}, act };
    return new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
  }

  it("the screens and the arrangement come from the desk", () => {
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("snapshot");
    const l = layoutFromDesk(snap);
    expect(l.arrangement).toEqual({ aSide: "right", align: "start" });
    expect(l.screens.map((s) => [s.id, s.machine, s.res, s.ratio, s.scale])).toEqual([
      ["a:Built-in", "a", "1512 × 945", "16:10", "200 %"],
      ["b:Odyssey", "b", "3440 × 1440", "21:9", "100 %"],
    ]);
    // Without declared screens: one per machine, with its name.
    const none = layoutFromDesk({ ...snap, desk: { ...snap.desk, screens: [], arrangement: null } });
    expect(none.screens.map((s) => s.name)).toEqual(["MacBook Pro", "PC-Desktop"]);
    expect(none.arrangement).toEqual({ aSide: "left", align: "center" });
  });

  /** The desk with the PC (b) with two connected monitors, side by side. */
  const twoScreens = (): Snapshot => {
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("snapshot");
    return {
      ...snap,
      desk: {
        ...snap.desk,
        screens: [
          { machine: "a", id: "Built-in", x: 0, y: 0, width: 1512, height: 945, scale: 2 },
          { machine: "b", id: "Odyssey", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
          { machine: "b", id: "Side", x: 3440, y: -200, width: 2560, height: 1440, scale: 1 },
        ],
      },
    };
  };

  it("a machine's screens carry their position on the desktop", () => {
    const snap = twoScreens();
    if (snap.kind !== "connected") throw new Error("snapshot");
    const l = layoutFromDesk(snap);
    const side = l.screens.find((s) => s.id === "b:Side");
    const main = l.screens.find((s) => s.id === "b:Odyssey");
    expect(side?.usable).toBe(true);
    expect(side && main && side.x - main.x).toBeCloseTo(3440 / 110, 9);
    expect(side?.y).toBeCloseTo(-200 / 110, 9);
  });

  it("a second connected screen is not \"Unsupported\" and edits the same arrangement", async () => {
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const bridgeTwo: Bridge = new Proxy({} as Bridge, {
      get: (_t, k) => ({ snapshot: async () => twoScreens(), onSnapshot: async () => {}, act })[k as string] ?? (() => new Promise(() => {})),
    });
    await open(new LiveSource(bridgeTwo));
    await userEvent.click(screen.getByRole("button", { name: /Side/ }));
    expect(screen.queryByText(/Unsupported/)).toBeNull();
    expect(pad()).toHaveAccessibleName("Where is Built-in?");
    await userEvent.click(slot("Top"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    // From b the MacBook is above: it is saved as A's side.
    expect(act).toHaveBeenCalledWith({ kind: "set_arrangement", a_side: "up", align: "start" });
  });

  it("on saving, the canvas does not go back to the old arrangement while the new one arrives", async () => {
    let push: (s: Snapshot) => void = () => {};
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const known: Partial<Record<keyof Bridge, unknown>> = {
      snapshot: async () => connected(),
      onSnapshot: async (cb: (s: Snapshot) => void) => {
        push = cb;
      },
      act,
    };
    const b = new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
    await open(new LiveSource(b));
    await userEvent.click(slot("Bottom"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(await screen.findByText("Arrangement saved")).toBeInTheDocument();
    // The agent answered, but the new `Desk` has not arrived: what was saved keeps showing.
    expect(slot("Bottom")).toHaveAttribute("aria-checked", "true");
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("snapshot");
    // A snapshot that repeats the old arrangement does not release the draft.
    push(snap);
    await tick();
    expect(slot("Bottom")).toHaveAttribute("aria-checked", "true");
    push({ ...snap, desk: { ...snap.desk, arrangement: { a_side: "down", align: "start" } } });
    await tick();
    expect(slot("Bottom")).toHaveAttribute("aria-checked", "true");
    // With the new arrangement already in the desk the draft was released: whatever arrives afterwards is seen.
    push({ ...snap, desk: { ...snap.desk, arrangement: { a_side: "left", align: "end" } } });
    await tick();
    expect(slot("Left")).toHaveAttribute("aria-checked", "true");
  });

  it("saving is a desk change over the IPC (`set_arrangement`)", async () => {
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    await open(new LiveSource(bridge(act)));
    await userEvent.click(slot("Bottom"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_arrangement", a_side: "down", align: "start" });
    expect(await screen.findByText("Arrangement saved")).toBeInTheDocument();
  });

  it("if the agent rejects, it says so and the change stays unsaved", async () => {
    const act = vi.fn(async () => ({ kind: "rejected" as const, reason: "machine b is not known yet" }));
    const store = await open(new LiveSource(bridge(act)));
    await userEvent.click(slot("Bottom"));
    await userEvent.click(screen.getByRole("button", { name: /Save/ }));
    expect(await screen.findByText("machine b is not known yet")).toBeInTheDocument();
    expect(slot("Bottom")).toHaveAttribute("aria-checked", "true");
    expect(store.desk.layout.arrangement.aSide).toBe("right");
  });

  it("the crossing test uses the crossing the agent reports: enabled button and \"Crossed fine\" through the right edge", async () => {
    let notice: ((n: Notice) => void) | undefined;
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const withNotice = (): Bridge => {
      const known: Partial<Record<keyof Bridge, unknown>> = {
        snapshot: async () => connected(),
        onSnapshot: async () => {},
        act,
        onNotice: async (cb: (n: Notice) => void) => {
          notice = cb;
        },
      };
      return new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
    };
    await open(new LiveSource(withNotice()));
    expect(screen.getByRole("link", { name: /Test crossing/ })).toBeInTheDocument();
    document.body.innerHTML = "";
    await open(new LiveSource(withNotice()), "#/arrangement/crossing");
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true }));
    // The arrangement says "A to the right of": the Mac leaves through its left edge.
    notice?.({ kind: "crossed", from: "a", to: "b", edge: "left" });
    notice?.({ kind: "cursor", sample: { machine: "b", screen: "Odyssey", x: 3439, y: 700 } });
    expect(await screen.findByText("Crossed fine")).toBeInTheDocument();
  });
});
