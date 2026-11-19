import { describe, expect, it } from "vitest";

import {
  STEPS,
  advanceTo,
  begin,
  canAdvance,
  countsAsKeyProbe,
  goBack,
  nextStep,
  shouldOpen,
  summaryChecks,
  viewOf,
  type Onboarding,
  type StepId,
} from "./onboarding";
import type { InputView, MachineCaps, PairingView, PeerView, Snapshot } from "./model";

const PROCESS = { exe: "/Applications/Tandem.app/Contents/MacOS/tandemd", pid: 4242, started: 1 };

// Sample data: A with macOS and B with Windows (it could be any combination, D2).
function caps(machine: "a" | "b", extra: Partial<MachineCaps> = {}): MachineCaps {
  const pc = machine === "b";
  return {
    machine,
    name: pc ? "DESKTOP" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    color: pc ? "#a99be8" : "#7aa7e6",
    hub: null,
    input: { capture: true, inject: true, missing: [], process: PROCESS },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
    ...extra,
  };
}

const INPUT: InputView = { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: false };
const PAIRED: PeerView = { kind: "paired", name: "DESKTOP", link: "usb4" };
const IDLE: PairingView = { kind: "idle" };

interface Parts {
  agent: "a" | "b";
  peer: PeerView;
  pairing: PairingView;
  here: MachineCaps;
  input: InputView;
}

function snap(over: Partial<Parts> = {}): Snapshot {
  const agent = over.agent ?? "a";
  return {
    kind: "connected",
    agent,
    desk: { machines: { a: null, b: null }, screens: [], arrangement: null, scenes: [] },
    state: { on: { a: true, b: true } },
    peer: over.peer ?? PAIRED,
    pairing: over.pairing ?? IDLE,
    input: over.input ?? INPUT,
    caps: { here: over.here ?? caps(agent), peer: caps(agent === "a" ? "b" : "a") },
    monitors: [],
  };
}

function at(step: StepId, patch: Partial<Onboarding> = {}): Onboarding {
  return { ...begin(step), ...patch };
}

const ids = (v: { buttons: { id: string }[] }) => v.buttons.map((b) => b.id);
const btn = (v: { buttons: { id: string; enabled: boolean }[] }, id: string) => v.buttons.find((b) => b.id === id);

// ---------------------------------------------------------------------------------------

describe("the steps", () => {
  it("there are four, without monitors, and in this order (DESIGN §5)", () => {
    expect(STEPS.map((s) => s.id)).toEqual(["machine", "pair", "keyboard", "summary"]);
  });

  it("opens by itself on a clean install: without a peer and without a declared keyboard", () => {
    expect(shouldOpen(snap({ peer: { kind: "unpaired" } }))).toBe(true);
    expect(shouldOpen(snap())).toBe(false);
    expect(shouldOpen(snap({ peer: { kind: "unpaired" }, input: { ...INPUT, defined: true } }))).toBe(false);
    expect(shouldOpen({ kind: "connecting" })).toBe(false);
  });
});

describe("step 1: this machine", () => {
  it("says how the machine was detected", () => {
    const v = viewOf(at("machine"), snap({ agent: "b" }));
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "DESKTOP", detail: expect.stringContaining("Windows") });
    expect(v.verified).toBe(true);
  });

  it("on macOS it verifies the permissions live: it does not advance while one is missing", () => {
    const missing = caps("a", { input: { capture: false, inject: false, missing: ["accessibility", "input_monitoring"], process: PROCESS } });
    const v = viewOf(at("machine"), snap({ here: missing }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("macOS permissions are missing");
    expect(ids(v)).toEqual(["perm:request", "perm:accessibility", "perm:input_monitoring", "perm:restart"]);
    expect(v.checks.filter((c) => c.state === "fail")).toHaveLength(2);
    // The names also come out as macOS calls them in English.
    expect(v.checks.some((c) => c.label === "Permission: Accessibility")).toBe(true);
    expect(v.checks.some((c) => c.label === "Permission: Input Monitoring")).toBe(true);
    // And which binary to grant them to, without opening a terminal.
    const binary = v.checks.find((c) => c.label === "Agent binary");
    expect(binary?.detail).toContain("/Applications/Tandem.app/Contents/MacOS/tandemd");
    expect(v.checks.some((c) => c.label === "How to grant" && (c.detail ?? "").includes("Restart agent"))).toBe(true);
    // One is granted: the other remains.
    const one = caps("a", { input: { capture: true, inject: true, missing: ["input_monitoring"], process: PROCESS } });
    expect(ids(viewOf(at("machine"), snap({ here: one })))).toEqual(["perm:request", "perm:input_monitoring", "perm:restart"]);
    // All are granted: it is checked by itself.
    const ok = viewOf(at("machine"), snap());
    expect(ok.verified).toBe(true);
    expect(ok.checks.some((c) => c.label === "macOS permissions" && c.state === "ok")).toBe(true);
    // With everything granted there is no offer to request or restart anything.
    expect(ids(ok)).toEqual([]);
  });

  it("with no agent there is nothing to verify", () => {
    const v = viewOf(at("machine"), { kind: "connecting" });
    expect(v.verified).toBe(false);
    expect(v.blocker).toContain("tandemd");
  });
});

describe("step 2: the other machine", () => {
  const unpaired: PeerView = { kind: "unpaired" };

  it("unpaired, you can search or continue alone for now", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired }));
    expect(ids(v)).toEqual(["pair:start", "pair:skip"]);
    expect(v.verified).toBe(false);
    const skipped = viewOf(at("pair", { pair: { ...begin().pair, skipped: true } }), snap({ peer: unpaired }));
    expect(skipped.verified).toBe(true);
    expect(skipped.checks.some((c) => c.label.includes("this machine alone"))).toBe(true);
  });

  it("shows the code and asks to compare it; you cannot skip it midway", () => {
    const comparing: PairingView = { kind: "comparing", code: "418027", peer: "DESKTOP", confirmed: false };
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: comparing }));
    expect(v.code).toBe("418027");
    expect(ids(v)).toEqual(["pair:confirm", "pair:cancel", "pair:skip"]);
    expect(btn(v, "pair:skip")?.enabled).toBe(false);
    const waiting = viewOf(at("pair"), snap({ peer: unpaired, pairing: { ...comparing, confirmed: true } }));
    expect(ids(waiting)).not.toContain("pair:confirm");
    const searching = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "searching" } }));
    expect(ids(searching)).toContain("pair:cancel");
  });

  it("a failed pairing says why", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "failed", reason: "rejected on DESKTOP" } }));
    expect(v.checks[0]).toMatchObject({ state: "fail", detail: "rejected on DESKTOP" });
  });

  it("paired but with the link unverified does not advance: the ping is needed", () => {
    const v = viewOf(at("pair"), snap());
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("test the link");
    expect(btn(v, "ping")).toMatchObject({ enabled: true, label: "Test the link" });
    const done = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap());
    expect(done.verified).toBe(true);
    expect(done.checks.find((c) => c.label === "Link ping")).toMatchObject({ state: "ok", detail: "Wi-Fi · 7 ms" });
  });

  it("paired but with no link cannot be verified", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap({ peer: { kind: "paired", name: "DESKTOP", link: null } }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("the other machine does not respond");
  });

  it("a failed ping is shown", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, pingError: "no link reaches the other machine" } }), snap());
    expect(v.checks.find((c) => c.label === "Link ping")).toMatchObject({ state: "fail" });
  });
});

describe("step 3: keyboard and mouse", () => {
  it("at first nothing is defined and a machine is never assumed", () => {
    const v = viewOf(at("keyboard"), snap());
    expect(v.verified).toBe(false);
    expect(ids(v)).toEqual(["key:start", "key:pick:a", "key:pick:b", "key:skip"]);
    expect(v.checks.some((c) => c.detail === "a machine is never assumed")).toBe(true);
  });

  it("listens on both machines while waiting for the key", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, armed: true } }), snap());
    expect(v.checks[0]).toMatchObject({ state: "wait" });
    expect(v.checks[0]!.label).toContain("both machines");
    expect(ids(v)).not.toContain("key:start");
  });

  it("the machine that received the key has it, but it has to be confirmed", () => {
    const heard = at("keyboard", { keyboard: { ...begin().keyboard, armed: true, heard: "b" } });
    const v = viewOf(heard, snap());
    expect(v.checks[0]!.label).toBe("The key arrived on DESKTOP");
    expect(ids(v)).toEqual(["key:yes:b", "key:yes:a", "key:again"]);
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("confirm where the keyboard is");
  });

  it("confirmed is verified", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, confirmed: "b" } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "The keyboard and mouse are on DESKTOP" });
  });

  it("with no reply it is explicitly left undefined", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, undefinedOk: true } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks.some((c) => c.label === "Undefined")).toBe(true);
  });

  it("without the other machine linked it warns that it only listens here", () => {
    const v = viewOf(at("keyboard"), snap({ peer: { kind: "unpaired" } }));
    expect(v.checks[0]!.detail).toContain("it only listens here");
  });
});

describe("step 4: summary", () => {
  it("gathers everything verified, with the name of each machine", () => {
    const o = at("summary", {
      pair: { ...begin().pair, ping: { link: "usb4", millis: 2 } },
      keyboard: { ...begin().keyboard, confirmed: "b" },
    });
    const text = summaryChecks(o, snap()).map((l) => `${l.state}|${l.label}|${l.detail ?? ""}`);
    expect(text).toEqual([
      "ok|MacBook-Pro|permissions verified",
      "ok|DESKTOP paired|USB-C · 2 ms",
      "ok|Keyboard and mouse on DESKTOP|confirmed by you",
    ]);
    expect(viewOf(o, snap()).buttons.map((b) => b.id)).toEqual(["finish"]);
  });

  it("what was not done is said: keyboard undefined, without the other machine", () => {
    const lines = summaryChecks(begin("summary"), snap({ peer: { kind: "unpaired" } })).map((l) => l.label);
    expect(lines).toContain("Keyboard and mouse undefined");
    expect(lines).toContain("Without the other machine");
  });
});

describe("navigation", () => {
  it("does not advance if the step is not verified", () => {
    const o = at("pair");
    expect(canAdvance(o, snap())).toBe(false);
    expect(advanceTo(o, snap())).toBe(o);
    const ok = at("pair", { pair: { ...begin().pair, ping: { link: "usb4", millis: 1 } } });
    expect(canAdvance(ok, snap())).toBe(true);
    expect(advanceTo(ok, snap()).step).toBe("keyboard");
  });

  it("you can go back to the first one, and on the summary you no longer advance", () => {
    expect(goBack(at("machine")).step).toBe("machine");
    expect(goBack(at("keyboard")).step).toBe("pair");
    expect(goBack(at("summary")).step).toBe("keyboard");
    expect(nextStep(at("keyboard"))).toBe("summary");
    expect(canAdvance(at("summary"), snap())).toBe(false);
  });

  it("a full walkthrough goes through the four steps", () => {
    let o = begin();
    const visited: StepId[] = [o.step];
    const facts: Record<StepId, Partial<Onboarding>> = {
      machine: {},
      pair: { pair: { ...begin().pair, ping: { link: "usb4", millis: 3 } } },
      keyboard: { keyboard: { ...begin().keyboard, confirmed: "b" } },
      summary: {},
    };
    for (let i = 0; i < 3; i++) {
      o = { ...o, ...facts[o.step] };
      o = advanceTo(o, snap());
      visited.push(o.step);
    }
    expect(visited).toEqual(["machine", "pair", "keyboard", "summary"]);
  });
});

describe("pressing a key", () => {
  it("counts any keyboard key, but not activating a dialog button", () => {
    expect(countsAsKeyProbe({ key: "a", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "a", targetIsButton: true })).toBe(true);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: " ", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Tab", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "Shift", targetIsButton: false })).toBe(false);
    expect(countsAsKeyProbe({ key: "a", repeat: true, targetIsButton: false })).toBe(false);
  });
});
