// The Tunnels screen's live output (the terminal view). Before: every desk update (a new state,
// a link switch, a resource sample) re-ran the screen's effects, which emptied the process list
// and the output and fetched them again from byte 0, and the output was polled once per second
// in 64 KiB pieces. These tests pin the fixed behavior: the output is pushed, only appended,
// and survives desk updates and link switches.
import { act as flush, render, screen } from "@testing-library/svelte";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { STORE_KEY } from "../lib/context";
import type { Action, ActionResult, Snapshot } from "../lib/model";
import { LiveSource } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import { continues, decodeBase64, type LiveOutputWire, type ProcessWire, type ProjectWire } from "../lib/tunnels";
import { escapeStripper } from "../lib/term";
import Shell from "../shell/Shell.svelte";

afterEach(() => {
  vi.useRealTimers();
});

const TUNNEL = "f00dfacecafe0001";

function snapshot(link: "usb4" | "wifi"): Snapshot {
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
        modules: [],
      },
      peer: null,
    },
    monitors: [],
  } as unknown as Snapshot;
}

const shell: ProcessWire = { id: 3, command: "pwsh", pid: 4242, started: Date.now() - 5_000, duration: null, state: { kind: "running" } };
const wire: ProjectWire[] = [{ name: "tandem", root: "/src/tandem", tunnels: [{ id: TUNNEL, title: "PC shell", origin: "a", target: "b", open: true, created: Date.now() - 60_000, running: [shell] }] }];

const b64 = (s: string) => btoa(String.fromCharCode(...new TextEncoder().encode(s)));

function output(from: number, text: string, done = false): LiveOutputWire {
  const len = new TextEncoder().encode(text).length;
  return { tunnel: TUNNEL, process: shell, pty: true, from_stdout: from, from_stderr: 0, next_stdout: from + len, next_stderr: 0, data: b64(text), done };
}

function rig() {
  let pushSnapshot: ((s: Snapshot) => void) | undefined;
  let pushTunnels: ((t: ProjectWire[]) => void) | undefined;
  let pushOutput: ((o: LiveOutputWire) => void) | undefined;
  const act = vi.fn(async (a: Action): Promise<ActionResult> => {
    if (a.kind === "tunnels") return { kind: "tunnels", tunnels: wire };
    if (a.kind === "processes") return { kind: "processes", processes: [shell] };
    return { kind: "done" };
  });
  const b = {
    snapshot: async () => snapshot("usb4"),
    onSnapshot: async (cb: (s: Snapshot) => void) => {
      pushSnapshot = cb;
    },
    onTunnels: async (cb: (t: ProjectWire[]) => void) => {
      pushTunnels = cb;
    },
    onOutput: async (cb: (o: LiveOutputWire) => void) => {
      pushOutput = cb;
    },
    act,
    shortcuts: async () => ({ bindings: { toggle_window: null }, failed: {}, warning: null, file: null }),
  } as unknown as Bridge;
  return {
    b,
    act,
    snapshot: (s: Snapshot) => pushSnapshot?.(s),
    tunnels: (t: ProjectWire[]) => pushTunnels?.(t),
    output: (o: LiveOutputWire) => pushOutput?.(o),
  };
}

async function open(b: Bridge) {
  const store = new Store(new LiveSource(b), { hash: "#/tunnels" });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const kinds = (act: ReturnType<typeof rig>["act"], kind: string) => act.mock.calls.filter(([a]) => a.kind === kind).map(([a]) => a);
const terminalText = () => screen.getByTestId("terminal").textContent ?? "";

async function settle() {
  for (let i = 0; i < 5; i++) {
    await flush(async () => {
      await Promise.resolve();
    });
  }
}

describe("the terminal view of a tunnel's process", () => {
  it("watches the output once and appends what is pushed", async () => {
    const r = rig();
    await open(r.b);
    await settle();
    expect(kinds(r.act, "watch_output")).toEqual([{ kind: "watch_output", tunnel: TUNNEL, process: 3, stdout: 0, stderr: 0 }]);
    await flush(async () => r.output(output(0, "PS C:\\> ")));
    await flush(async () => r.output(output(8, "dir\r\n")));
    expect(terminalText()).toContain("PS C:\\> dir");
    // Pushed, not polled.
    expect(kinds(r.act, "output")).toEqual([]);
  });

  it("desk updates and link switches neither clear it nor restart it from zero", async () => {
    const r = rig();
    await open(r.b);
    await settle();
    await flush(async () => r.output(output(0, "line one\r\n")));
    const processQueries = kinds(r.act, "processes").length;
    for (const link of ["wifi", "usb4", "wifi", "usb4"] as const) {
      await flush(async () => r.snapshot(snapshot(link)));
      await flush(async () => r.tunnels(structuredClone(wire)));
      expect(screen.queryByText("Nothing running right now")).toBeNull();
      expect(terminalText()).toContain("line one");
    }
    await settle();
    // Still the first and only watch, from byte 0; the list was not emptied and fetched again.
    expect(kinds(r.act, "watch_output")).toHaveLength(1);
    expect(kinds(r.act, "processes").length).toBe(processQueries);
    await flush(async () => r.output(output(10, "line two\r\n")));
    expect(terminalText()).toContain("line one\nline two");
  });

  it("a stale piece from before is not shown twice", async () => {
    const r = rig();
    await open(r.b);
    await settle();
    await flush(async () => r.output(output(0, "abc")));
    await flush(async () => r.output(output(0, "abc")));
    await flush(async () => r.output(output(3, "def")));
    expect(terminalText()).toContain("abcdef");
    expect(terminalText()).not.toContain("abcabc");
  });
});

describe("live output helpers", () => {
  it("decodes base64 to the raw bytes, also a character split in two pieces", () => {
    const euro = new TextEncoder().encode("€");
    expect([...decodeBase64(btoa(String.fromCharCode(...euro)))]).toEqual([...euro]);
    const d = new TextDecoder();
    const text = d.decode(euro.slice(0, 2), { stream: true }) + d.decode(euro.slice(2), { stream: true });
    expect(text).toBe("€");
  });

  it("a piece continues the cursor unless it ends before it", () => {
    const c = (from: number, next: number) => ({ bytes: new Uint8Array(), from: { stdout: from, stderr: 0 }, next: { stdout: next, stderr: 0 }, done: false, pty: true });
    expect(continues({ stdout: 0, stderr: 0 }, c(0, 3))).toBe(true);
    expect(continues({ stdout: 3, stderr: 0 }, c(0, 3))).toBe(false);
    expect(continues({ stdout: 3, stderr: 0 }, c(10, 12))).toBe(true);
  });

  it("strips escape sequences, also when split across pieces", () => {
    const strip = escapeStripper();
    expect(strip("\x1b[32mok\x1b[0m done\x1b]0;title\x07!")).toBe("ok done!");
    expect(strip("a\x1b[3")).toBe("a");
    expect(strip("1mb")).toBe("b");
  });
});
