import { expect, it } from "vitest";

import type { SetupView } from "./model";
import { PERMISSION_INFO, back, isFirst, isLast, next, screensOf, start } from "./setup";

const VIEW: SetupView = {
  available: true,
  show: true,
  permissions: ["accessibility", "input_monitoring", "local_network"],
  agent: { ok: true, text: "tandemd quedó instalado" },
};

it("tiene una bienvenida y una pantalla por permiso, en el orden del núcleo", () => {
  expect(screensOf(VIEW)).toEqual([
    { kind: "intro" },
    { kind: "permission", permission: "accessibility" },
    { kind: "permission", permission: "input_monitoring" },
    { kind: "permission", permission: "local_network" },
  ]);
});

it("avanza y retrocede sin salirse", () => {
  let w = start(VIEW);
  expect(w).toEqual({ index: 0, total: 4 });
  expect(isFirst(w)).toBe(true);
  expect(back(w)).toEqual(w);
  w = next(next(next(w)));
  expect(isLast(w)).toBe(true);
  expect(next(w)).toEqual(w);
  expect(back(w).index).toBe(2);
});

it("sin permisos que pedir, la bienvenida es la última", () => {
  const w = start({ ...VIEW, permissions: [] });
  expect(isFirst(w) && isLast(w)).toBe(true);
});

it("explica cada permiso", () => {
  for (const p of VIEW.permissions) {
    const info = PERMISSION_INFO[p];
    expect(info.title).not.toBe("");
    expect(info.why).not.toBe("");
    expect(info.how).not.toBe("");
  }
});
