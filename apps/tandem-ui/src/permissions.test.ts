import { expect, it } from "vitest";

import type { MachineCaps } from "./model";
import { MISSING_HELP, PERMISSION_NAMES, agentBinary, grantTo, permissionLabel, permissionState, pressPermission } from "./permissions";

function mac(exe: string): MachineCaps {
  return {
    machine: "mac",
    name: "MacBook-Pro",
    os: "macos",
    scanned: true,
    monitors: [],
    builtin: true,
    hub: null,
    input: { capture: false, inject: false, missing: ["accessibility"], process: { exe, pid: 4242, started: 1 } },
    power: { wake_peer: true, wakeable: true, power_off: false },
    services: [],
  };
}

it("nombra cada permiso en español y como lo llama macOS en inglés", () => {
  expect(permissionLabel("accessibility")).toBe("Accesibilidad (Accessibility)");
  expect(permissionLabel("input_monitoring")).toBe("Monitorización de entrada (Input Monitoring)");
  expect(permissionLabel("local_network")).toBe("Red local (Local Network)");
  expect(Object.keys(PERMISSION_NAMES)).toHaveLength(3);
});

it("dice a qué binario se concede, con la ruta real que anuncia el agente", () => {
  const here = mac("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(agentBinary(here)).toBe("/Applications/Tandem.app/Contents/MacOS/tandemd");
  expect(grantTo(here)).toBe("Concede el permiso a este binario: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 4242)");
});

it("sin ruta (agente que no la dio) o sin agente no inventa ninguna", () => {
  expect(grantTo(mac(""))).toBeNull();
  expect(agentBinary(null)).toBeNull();
  expect(grantTo(null)).toBeNull();
});

it("la ayuda cubre pedir, conceder, reiniciar y la entrada vieja", () => {
  expect(MISSING_HELP).toContain("Pedir permisos");
  expect(MISSING_HELP).toContain("Reiniciar agente");
  expect(MISSING_HELP).toContain("Privacy & Security");
  expect(MISSING_HELP).toContain("entrada vieja");
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

it("«Pedir permisos» devuelve la razón si el agente la rechaza", async () => {
  expect(await pressPermission("request", deps())).toEqual({ error: null });
  const r = await pressPermission("request", deps({ request: async () => ({ kind: "rejected" as const, reason: "no" }) }));
  expect(r.error).toBe("no");
});

it("«Reiniciar agente» avisa solo si de verdad reinició", async () => {
  const ok = await pressPermission("restart", deps());
  expect(ok.error).toBeNull();
  expect(ok.message).toContain("se reinicia");
  const none = await pressPermission("restart", deps({ restartAgent: null }));
  expect(none.message).toBeUndefined();
  expect(none.error).toContain("No se pudo reiniciar");
  const bad = await pressPermission("restart", deps({ restartAgent: async () => { throw new Error("launchctl"); } }));
  expect(bad.message).toBeUndefined();
  expect(bad.error).toContain("launchctl");
});

it("cualquier otro botón de permisos abre el panel de Ajustes", async () => {
  let opened = 0;
  await pressPermission("accessibility", deps({ openSettings: async () => { opened++; } }));
  expect(opened).toBe(1);
});
