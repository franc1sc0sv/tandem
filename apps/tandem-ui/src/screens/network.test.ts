import { readFileSync } from "node:fs";
import { join } from "node:path";
import { act as flush, fireEvent, render, screen, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import type { NetworkWire } from "../lib/network";
import { DEMO_NOW, DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import Shell from "../shell/Shell.svelte";

afterEach(() => {
  vi.useRealTimers();
});

async function open(source: Source) {
  const store = new Store(source, { hash: "#/network" });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const demo = (q = "demo&still") => open(new DemoSource(demoOptions(q)));
const articles = () => [...document.querySelectorAll("article")];

describe("Network (V3-Network) in demo mode", () => {
  it("draws the map: the two machines with their color and the three paths", async () => {
    await demo();
    expect(screen.getByRole("heading", { name: "Network", level: 1 })).toBeInTheDocument();
    const map = screen.getByRole("region", { name: "Link map" });
    expect(within(map).getByText("MacBook Pro")).toBeInTheDocument();
    expect(within(map).getByText("PC-Desktop")).toBeInTheDocument();
    const paths = within(map).getByRole("img", { name: "Three paths: USB-C active at 0.3 ms, Ethernet no cable, Wi-Fi as backup at 3.8 ms" });
    expect(within(paths).getByText("USB-C")).toBeInTheDocument();
    expect(within(paths).getByText("0.3 ms")).toBeInTheDocument();
    expect(within(paths).getByText("Ethernet · no cable")).toBeInTheDocument();
    expect(within(paths).getByText("3.8 ms")).toBeInTheDocument();
    // The halo carries each machine's color.
    const halos = [...map.querySelectorAll<HTMLElement>(".node")].map((n) => n.style.getPropertyValue("--c"));
    expect(halos).toEqual(["#7aa7e6", "#a99be8"]);
    // The path that flows is the cable's and Wi-Fi's; Ethernet is dotted.
    expect([...map.querySelectorAll(".flow")].map((p) => p.classList[1])).toEqual(["usb4", "wifi"]);
    expect(map.querySelectorAll('path[stroke-dasharray="4 7"]')).toHaveLength(1);
  });

  it("the indicators: latency p50 and p99, bandwidth and the chart with the peak and its note", async () => {
    await demo();
    expect(screen.getByText("Keyboard and mouse")).toBeInTheDocument();
    expect(screen.getByText("0.4 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    const bw = screen.getAllByText("Bandwidth").find((e) => e.classList.contains("lbl"))!.closest<HTMLElement>(".card")!;
    expect(within(bw).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(within(bw).getByText("over USB-C")).toBeInTheDocument();
    expect(screen.getByText("Latency · 60 s")).toBeInTheDocument();
    expect(screen.getByText("target 4 ms")).toBeInTheDocument();
    expect(screen.getByRole("img", { name: "Latency over the last 60 s: now 1.3 ms, peak of 5.6 ms" })).toBeInTheDocument();
    expect(document.querySelectorAll("circle.pk")).toHaveLength(1);
    expect(screen.getByText("Spike when the cable was unplugged · switched to Wi-Fi in 0.6 s")).toBeInTheDocument();
  });

  it("one card per path, with its uniform status chip and its bars", async () => {
    await demo();
    const [usb, eth, wifi] = articles();
    expect(within(usb!).getByText("USB-C cable")).toBeInTheDocument();
    expect(within(usb!).getByText("USB4 · 20 Gbps")).toBeInTheDocument();
    expect(usb!.querySelector(".st")).toHaveClass("primary");
    expect(within(usb!).getByText("Primary")).toBeInTheDocument();
    expect(within(usb!).getByText("0.3 ms")).toBeInTheDocument();
    expect(within(usb!).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(within(usb!).getByRole("meter", { name: "USB-C bandwidth" })).toHaveAttribute("aria-valuenow", "94");
    expect(within(eth!).getByText("Ethernet")).toBeInTheDocument();
    expect(within(eth!).getByText("No cable")).toBeInTheDocument();
    expect(eth!.querySelector(".st")).toHaveClass("unplugged");
    expect(eth!.querySelectorAll(".val")[0]).toHaveTextContent("—");
    expect(within(eth!).queryByRole("meter")).toBeNull();
    expect(within(wifi!).getByText("Wi-Fi 7 · home")).toBeInTheDocument();
    expect(wifi!.querySelector(".st")).toHaveClass("backup");
    expect(within(wifi!).getByText("Backup")).toBeInTheDocument();
    expect(within(wifi!).getByText("410 Mbps")).toBeInTheDocument();
    expect(within(wifi!).getByRole("meter", { name: "Wi-Fi latency" })).toHaveAttribute("aria-valuenow", "38");
  });

  it("there are no filler texts or template leftovers", async () => {
    await demo();
    const text = document.body.innerText ?? document.body.textContent ?? "";
    for (const bad of ["{{", "undefined", "NaN", "Add machine", "Simulate", "lorem"]) expect(text).not.toContain(bad);
    // Without the search from other screens: V3-Network does not have it.
    expect(screen.queryByText("Search or run…")).toBeNull();
  });

  it("measuring bandwidth measures the primary path, announces the value and cannot be repeated while measuring", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const src = new DemoSource(demoOptions("demo"));
    const spy = vi.spyOn(src, "measureLink");
    await open(src);
    const btn = screen.getByRole("button", { name: "Measure bandwidth" });
    expect(btn).toBeEnabled();
    await fireEvent.click(btn);
    expect(spy).toHaveBeenCalledTimes(1);
    expect(spy).toHaveBeenCalledWith(undefined);
    // The test lasts: the button turns off and says which path is being measured.
    const busy = await screen.findByRole("button", { name: "Measuring USB-C…" });
    expect(busy).toBeDisabled();
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(1600);
    });
    expect(await screen.findByText("USB-C: 9.4 Gbps")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeEnabled();
  });

  it("no link with the other machine: the three paths with no cable and the button off", async () => {
    await demo("demo&still&trouble=link-down");
    expect(screen.getAllByText("No cable")).toHaveLength(2);
    expect(screen.getAllByText("No signal")).toHaveLength(1);
    expect(screen.getByText("Wi-Fi · no signal")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/Wi-Fi[^.]{0,6}no cable/i);
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeDisabled();
    expect(screen.getByText("no input")).toBeInTheDocument();
    expect(screen.getByText("not measured")).toBeInTheDocument();
    expect(screen.getByText("No samples yet")).toBeInTheDocument();
    expect(document.querySelectorAll("circle.pk")).toHaveLength(0);
    expect(document.querySelectorAll(".flow")).toHaveLength(0);
  });

  it("with the screen visible, the history advances and the note appears when the cut leaves the chart", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const store = await open(new DemoSource(demoOptions("demo")));
    expect(screen.getByText(/^Spike when the cable was unplugged/)).toBeInTheDocument();
    await flush(async () => {
      await vi.advanceTimersByTimeAsync(1500 * 27);
    });
    expect(store.desk.network.now).toBeGreaterThan(DEMO_NOW + 24_000);
    expect(screen.queryByText(/^Spike when the cable was unplugged/)).toBeNull();
    expect(screen.getByText("Switched from Wi-Fi to USB-C")).toBeInTheDocument();
  });
});

// ---------------------------------------------------------------------------------------
// With the real agent: the IPC (the bridge) instead of the demo.

function snapshot(link: "usb4" | "wifi" | null = "usb4"): Snapshot {
  return {
    kind: "connected",
    agent: "a",
    desk: { machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Desktop", color: "#a99be8" } }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Desktop", link },
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
  };
}

const wire = (over: Partial<NetworkWire> = {}): NetworkWire => ({
  paths: [
    { link: "usb4", state: "primary", rtt_us: 300, input_p50_us: 400, input_p99_us: 1900, input_target_us: 4000, bytes_per_sec: 1_175_000_000, history: [{ rtt_us: 300, input_p99_us: 1900 }] },
    { link: "ethernet", state: "unplugged", rtt_us: null, input_p50_us: null, input_p99_us: null, input_target_us: 4000, bytes_per_sec: null, history: [] },
    { link: "wifi", state: "backup", rtt_us: 3800, input_p50_us: null, input_p99_us: null, input_target_us: 20_000, bytes_per_sec: null, history: [{ rtt_us: 3800, input_p99_us: null }] },
  ],
  switches: [],
  measuring: null,
  ...over,
});

function bridge(measure: ActionResult = { kind: "bandwidth", link: "usb4", bytes_per_sec: 1_175_000_000 }) {
  const handlers: { network?: (w: NetworkWire) => void; snapshot?: (s: Snapshot) => void } = {};
  const act = vi.fn(async (a: Action): Promise<ActionResult> => (a.kind === "measure_link" ? measure : { kind: "done" }));
  const b = {
    snapshot: async () => snapshot(),
    act,
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      handlers.snapshot = cb;
    },
    onNetwork: async (cb: (w: NetworkWire) => void) => {
      handlers.network = cb;
    },
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
  } as unknown as Bridge;
  return { b, act, handlers };
}

describe("Network with the real agent (IPC)", () => {
  it("on opening the screen it asks the agent for the network and on leaving it stops asking", async () => {
    const { b, act } = bridge();
    const store = await open(new LiveSource(b));
    expect(act.mock.calls.filter(([a]) => a.kind === "watch_network").map(([a]) => a)).toEqual([{ kind: "watch_network", on: true }]);
    store.go({ screen: "settings" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: false });
    store.go({ screen: "network" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: true });
    // Asking for the same again does not repeat the request.
    const n = act.mock.calls.length;
    store.go({ screen: "network" });
    expect(act.mock.calls.length).toBe(n);
  });

  it("T-356: the Desk asks for the network too and its link bar shows the agent's measurements, never a dash", async () => {
    const { b, act, handlers } = bridge();
    const store = await open(new LiveSource(b));
    store.go({ screen: "settings" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: false });
    store.go({ screen: "desk" });
    expect(act).toHaveBeenLastCalledWith({ kind: "watch_network", on: true });
    await tick();
    // Nothing measured yet: zeros, not dashes, and the title says why.
    const bar = () => document.querySelector<HTMLElement>(".card.link")!;
    expect(bar().textContent).not.toMatch(/[–—]/);
    expect(within(bar()).getByText("0 Gbps · 0 ms")).toBeInTheDocument();
    expect(within(bar()).getByText("p99 0 ms")).toBeInTheDocument();
    // The pings, the last bandwidth test and, in a second without input, the newest p99 of the history.
    const w = wire();
    w.paths[0] = { ...w.paths[0]!, input_p50_us: null, input_p99_us: null, history: [{ rtt_us: 300, input_p99_us: 2100 }, { rtt_us: 300, input_p99_us: null }] };
    await flush(() => handlers.network?.(w));
    expect(within(bar()).getByText("9.4 Gbps · 0.3 ms")).toBeInTheDocument();
    expect(within(bar()).getByText("p99 2.1 ms")).toBeInTheDocument();
    expect(bar().querySelector(".stats")?.getAttribute("title")).toBeNull();
  });

  it("paints what the agent sends, every second, with the names and colors of each machine", async () => {
    const { b, handlers } = bridge();
    await open(new LiveSource(b));
    // Before the first sample: the three with no cable.
    expect(screen.getAllByText("No cable")).toHaveLength(2);
    expect(screen.getAllByText("No signal")).toHaveLength(1);
    await flush(() => handlers.network?.(wire({ switches: [] })));
    expect(screen.getByText("Primary")).toBeInTheDocument();
    expect(screen.getByText("0.4 ms")).toBeInTheDocument();
    expect(screen.getByText("p99 1.9 ms")).toBeInTheDocument();
    expect(within(screen.getAllByText("Bandwidth").find((e) => e.classList.contains("lbl"))!.closest<HTMLElement>(".card")!).getByText("9.4 Gbps")).toBeInTheDocument();
    expect(screen.getByText("Backup")).toBeInTheDocument();
    // Without link detail, the line is not invented.
    expect(screen.queryByText(/USB4 ·/)).toBeNull();
    expect(screen.getByText("MacBook Pro", { selector: ".node b" })).toBeInTheDocument();
    // The cable goes down: the agent says so and the screen follows it.
    const cut = wire();
    cut.paths[0] = { ...cut.paths[0]!, state: "down", rtt_us: null, input_p50_us: null, input_p99_us: null, bytes_per_sec: null };
    cut.paths[2] = { ...cut.paths[2]!, state: "primary" };
    cut.switches = [{ at_ms: Date.now() - 1000, from: "usb4", to: "wifi", took_ms: 600 }];
    await flush(() => handlers.network?.(cut));
    expect(screen.getByText("Down")).toBeInTheDocument();
    expect(screen.getByText("USB-C · down")).toBeInTheDocument();
    expect(screen.getByText("The cable was unplugged · switched to Wi-Fi in 0.6 s")).toBeInTheDocument();
  });

  it("the link notices use the warning pattern (icon and text) and never mention tandem.toml (T-243)", async () => {
    const { b, handlers } = bridge();
    await open(new LiveSource(b));
    const notices = [
      { interface: "Ethernet path", message: "Two addresses reach the other machine over the Ethernet path. Tandem uses [fe80::2%3]:47770 and ignores 169.254.252.182:47770.", command: "" },
      { interface: "Link manager", message: "The link manager is not running, so there is no link with the other machine: the health check timings cannot be 0", command: "" },
    ];
    await flush(() => handlers.network?.(wire({ notices })));
    const boxes = screen.getAllByRole("alert");
    expect(boxes).toHaveLength(2);
    for (const box of boxes) {
      expect(box.querySelector("svg")).not.toBeNull();
      expect(box.textContent).not.toMatch(/tandem\.toml|\[network\]/);
    }
    expect(within(boxes[1]!).getByText("Link manager")).toBeInTheDocument();
    expect(within(boxes[1]!).getByText(/^The link manager is not running/)).toBeInTheDocument();
  });

  it("\"Measure bandwidth\" sends MeasureLink through the primary path and announces what was measured", async () => {
    const { b, act, handlers } = bridge();
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire()));
    await userEvent.click(screen.getByRole("button", { name: "Measure bandwidth" }));
    expect(act).toHaveBeenCalledWith({ kind: "measure_link", path: "usb4" });
    expect(await screen.findByText("USB-C: 9.4 Gbps")).toBeInTheDocument();
  });

  it("if the agent rejects the test, it says why", async () => {
    const { b, handlers } = bridge({ kind: "rejected", reason: "the input went past its target" });
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire()));
    await userEvent.click(screen.getByRole("button", { name: "Measure bandwidth" }));
    expect(await screen.findByText("the input went past its target")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeEnabled();
  });

  it("if the agent is measuring (another window), the button says so and waits", async () => {
    const { b, handlers } = bridge();
    await open(new LiveSource(b));
    await flush(() => handlers.network?.(wire({ measuring: "wifi" })));
    expect(screen.getByRole("button", { name: "Measuring Wi-Fi…" })).toBeDisabled();
  });

  it("says when macOS blocks this Mac's Local Network, with where to turn it on (T-344)", async () => {
    const { b, handlers } = bridge();
    type Connected = Extract<Snapshot, { kind: "connected" }>;
    const denied = (): Connected => {
      const s = snapshot(null) as Connected;
      return { ...s, caps: { ...s.caps, here: { ...s.caps.here, local_network: "denied" } } };
    };
    (b.snapshot as unknown) = async () => denied();
    await open(new LiveSource(b));
    const alert = screen.getByRole("alert", { name: "Local Network" });
    expect(within(alert).getByText("Local Network permission missing on this Mac")).toBeInTheDocument();
    expect(within(alert).getByText(/System Settings → Privacy & Security → Local Network/)).toBeInTheDocument();
    const granted = denied();
    await flush(() => handlers.snapshot?.({ ...granted, caps: { ...granted.caps, here: { ...granted.caps.here, local_network: "granted" } } }));
    expect(screen.queryByRole("alert", { name: "Local Network" })).toBeNull();
  });

  it("without the other machine paired it cannot measure", async () => {
    const { b } = bridge();
    (b.snapshot as unknown) = async () => ({ ...snapshot(), peer: { kind: "unpaired" } });
    await open(new LiveSource(b));
    expect(screen.getByRole("button", { name: "Measure bandwidth" })).toBeDisabled();
  });
});

// ---------------------------------------------------------------------------------------
// Animations (G7) and prefers-reduced-motion (G8).

const SRC = join(process.cwd(), "src");
const read = (...p: string[]) => readFileSync(join(SRC, ...p), "utf8");
const MOCKUP = readFileSync(join(process.cwd(), "..", "..", "mockups", "v03", "V3-Network.dc.html"), "utf8");

describe("V3-Network animations", () => {
  const files = {
    map: read("screens", "network", "LinkMap.svelte"),
    kpis: read("screens", "network", "Indicators.svelte"),
    cards: read("screens", "network", "PathCard.svelte"),
    card: read("ui", "Card.svelte"),
    bar: read("ui", "Bar.svelte"),
    ring: read("ui", "Ring.svelte"),
    button: read("ui", "Button.svelte"),
  };

  it("each mockup @keyframes has its equivalent in the app", () => {
    const names = [...MOCKUP.matchAll(/@keyframes (\w+)/g)].map((m) => m[1]).sort();
    expect(names).toEqual(["dr", "fa", "grow", "halo", "pg", "pk", "ring", "up"]);
    // Where the equivalent of each lives (component names change, the effect does not).
    expect(files.card).toContain("@keyframes card-up");
    expect(files.map).toContain("@keyframes halo");
    for (const k of ["fa-usb", "fa-eth", "fa-wifi"]) expect(files.map).toContain(`@keyframes ${k}`);
    expect(files.cards).toContain("@keyframes pg");
    expect(files.bar).toContain("@keyframes bar-grow");
    expect(files.ring).toContain("@keyframes ring-in");
    expect(files.kpis).toContain("@keyframes dr");
    expect(files.kpis).toContain("@keyframes pk");
  });

  it("with the same durations and curves", () => {
    expect(files.map).toContain("animation: halo 2.6s ease-in-out infinite");
    expect(files.map).toContain("animation: fa-usb 0.8s linear infinite");
    expect(files.map).toContain("animation: fa-eth 0.8s linear infinite");
    expect(files.map).toContain("animation: fa-wifi 2.4s linear infinite");
    expect(files.card).toContain("card-up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both");
    expect(files.ring).toContain("ring-in 1.4s cubic-bezier(0.2, 0.8, 0.2, 1) both");
    expect(files.kpis).toContain("animation: dr 2s ease-out 0.2s forwards");
    expect(files.kpis).toContain("animation: pk 2s ease-in-out infinite");
    expect(files.cards).toContain("animation: pg 1.8s infinite");
    // Staggered entrance of the cards, like the mockup (0.06, 0.1, 0.14 and 0.18…0.26 s).
    expect(MOCKUP).toContain("animation-delay:.06s");
    expect(read("screens", "Network.svelte")).toContain("0.18 + i * 0.04");
    expect(files.kpis).toContain("delay={0.06}");
    expect(files.kpis).toContain("delay={0.1}");
    expect(files.kpis).toContain("delay={0.14}");
  });

  it("and the transitions: cards, labels, icons and button", () => {
    expect(files.card).toMatch(/border-color 0\.25s,\s*transform 0\.25s/);
    expect(files.map).toContain("transition: transform 0.25s");
    expect(files.map).toContain("scale(1.05)");
    expect(files.cards).toContain("transition: transform 0.3s cubic-bezier(0.3, 1.5, 0.5, 1)");
    expect(files.cards).toContain("rotate(-8deg) scale(1.08)");
    expect(files.cards).toContain("translateY(-3px)");
    expect(files.button).toContain("rotate(-25deg) scale(1.1)");
  });

  it("with prefers-reduced-motion everything turns off: the global rule and the already drawn chart line", () => {
    const css = read("styles", "global.css");
    const rule = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)?.[1] ?? "";
    expect(rule).toContain("animation: none !important");
    expect(rule).toContain("transition: none !important");
    // The line starts hidden (dashoffset 900) and `dr` animates it: without animation it has to be seen whole.
    const reduce = /@media \(prefers-reduced-motion: reduce\) \{\s*\.ln \{\s*stroke-dashoffset: 0;/.test(files.kpis);
    expect(reduce).toBe(true);
    // No JS animates here: it is all CSS, which that rule turns off.
    for (const src of Object.values(files)) expect(src).not.toMatch(/\.animate\(|svelte\/transition/);
  });
});
