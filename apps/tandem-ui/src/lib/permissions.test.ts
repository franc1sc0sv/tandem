import { expect, it } from "vitest";

import type { MachineCaps } from "./model";
import { MISSING_HELP, PERMISSION_NAMES, agentBinary, grantTo, permissionLabel, permissionState, pressPermission } from "./permissions";

function mac(exe: string): MachineCaps {
  return {
    machine: "a",
    name: "MacBook-Pro",
    os: "macos",
    color: "#7aa7e6",
    hub: null,
    input: { capture: false, inject: false, missing: ["accessibility"], process: { exe, pid: 4242, started: 1 } },
    power: { wake_peer: true, wakeable: true, power_off: false },
    services: [],
  };
}

it("names each permission in English the way macOS calls it", () => {
  expect(permissionLabel("accessibility")).toBe("Accessibility");
  expect(permissionLabel("input_monitoring")).toBe("Input Monitoring");
  expect(permissionLabel("local_network")).toBe("Local Network");
  expect(Object.keys(PERMISSION_NAMES)).toHaveLength(3);
});

it("says which binary it is granted to, with the real path the agent announces", () => {
  const here = mac("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(agentBinary(here)).toBe("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(grantTo(here)).toBe("Grant the permission to this binary: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 4242)");
});

it("without a path (an agent that did not give it) or with no agent, none is invented", () => {
  expect(grantTo(mac(""))).toBeNull();
  expect(agentBinary(null)).toBeNull();
  expect(grantTo(null)).toBeNull();
});

it("the help covers requesting, granting, restarting and the old entry", () => {
  expect(MISSING_HELP).toContain("Request permissions");
  expect(MISSING_HELP).toContain("Restart agent");
  expect(MISSING_HELP).toContain("Privacy & Security");
  expect(MISSING_HELP).toContain("remove the old entry");
});

it("the state of a permission comes from what the agent announces", () => {
  const here = mac("/x/tandemd");
  expect(permissionState(here, "accessibility")).toBe("missing");
  expect(permissionState(here, "input_monitoring")).toBe("granted");
  // Local network is asked by macOS itself; the agent does not check it.
  expect(permissionState(here, "local_network")).toBe("unknown");
  expect(permissionState(null, "accessibility")).toBe("unknown");
});

const deps = (over: Partial<Parameters<typeof pressPermission>[1]> = {}) => ({
  request: async () => ({ kind: "done" as const }),
  restartAgent: async () => {},
  openSettings: async () => {},
  ...over,
});

it("\"Request permissions\" returns the reason if the agent rejects it", async () => {
  expect(await pressPermission("request", deps())).toEqual({ error: null });
  const r = await pressPermission("request", deps({ request: async () => ({ kind: "rejected" as const, reason: "no" }) }));
  expect(r.error).toBe("no");
});

it("\"Restart agent\" warns only if it really restarted", async () => {
  const ok = await pressPermission("restart", deps());
  expect(ok.error).toBeNull();
  expect(ok.message).toContain("is restarting");
  const none = await pressPermission("restart", deps({ restartAgent: null }));
  expect(none.message).toBeUndefined();
  expect(none.error).toContain("Could not restart the agent");
  const bad = await pressPermission("restart", deps({ restartAgent: async () => { throw new Error("launchctl"); } }));
  expect(bad.message).toBeUndefined();
  expect(bad.error).toContain("launchctl");
});

it("any other permission button opens the Settings panel", async () => {
  let opened = 0;
  await pressPermission("accessibility", deps({ openSettings: async () => { opened++; } }));
  expect(opened).toBe(1);
});
