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

it("nombra cada permiso en español y como lo llama macOS en inglés", () => {
  expect(permissionLabel("accessibility")).toBe("Accessibility");
  expect(permissionLabel("input_monitoring")).toBe("Input Monitoring");
  expect(permissionLabel("local_network")).toBe("Local Network");
  expect(Object.keys(PERMISSION_NAMES)).toHaveLength(3);
});

it("dice a qué binario se concede, con la ruta real que anuncia el agente", () => {
  const here = mac("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(agentBinary(here)).toBe("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(grantTo(here)).toBe("Grant the permission to this binary: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 4242)");
});

it("sin ruta (agente que no la dio) o sin agente no inventa ninguna", () => {
  expect(grantTo(mac(""))).toBeNull();
  expect(agentBinary(null)).toBeNull();
  expect(grantTo(null)).toBeNull();
});

it("la ayuda cubre pedir, conceder, reiniciar y la entrada vieja", () => {
  expect(MISSING_HELP).toContain("Request permissions");
  expect(MISSING_HELP).toContain("Restart agent");
  expect(MISSING_HELP).toContain("Privacy & Security");
  expect(MISSING_HELP).toContain("remove the old entry");
});

it("el estado de un permiso sale de lo que anuncia el agente", () => {
  const here = mac("/x/tandemd");
  expect(permissionState(here, "accessibility")).toBe("missing");
  expect(permissionState(here, "input_monitoring")).toBe("granted");
  // La red local la pregunta macOS solo; el agente no la comprueba.
  expect(permissionState(here, "local_network")).toBe("unknown");
  expect(permissionState(null, "accessibility")).toBe("unknown");
});

const deps = (over: Partial<Parameters<typeof pressPermission>[1]> = {}) => ({
  request: async () => ({ kind: "done" as const }),
  restartAgent: async () => {},
  openSettings: async () => {},
  ...over,
});

it("«Request permissions» devuelve la razón si el agente la rechaza", async () => {
  expect(await pressPermission("request", deps())).toEqual({ error: null });
  const r = await pressPermission("request", deps({ request: async () => ({ kind: "rejected" as const, reason: "no" }) }));
  expect(r.error).toBe("no");
});

it("«Restart agent» avisa solo si de verdad reinició", async () => {
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

it("cualquier otro botón de permisos abre el panel de Settings", async () => {
  let opened = 0;
  await pressPermission("accessibility", deps({ openSettings: async () => { opened++; } }));
  expect(opened).toBe(1);
});
