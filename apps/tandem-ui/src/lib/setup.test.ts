import { expect, it } from "vitest";

import type { SetupView } from "./model";
import { PERMISSION_INFO, back, isFirst, isLast, next, screensOf, start } from "./setup";

const VIEW: SetupView = {
  available: true,
  show: true,
  permissions: ["accessibility", "input_monitoring", "local_network"],
  agent: { ok: true, text: "tandemd is installed" },
};

it("has a welcome and one screen per permission, in the core's order", () => {
  expect(screensOf(VIEW)).toEqual([
    { kind: "intro" },
    { kind: "permission", permission: "accessibility" },
    { kind: "permission", permission: "input_monitoring" },
    { kind: "permission", permission: "local_network" },
  ]);
});

it("advances and goes back without leaving", () => {
  let w = start(VIEW);
  expect(w).toEqual({ index: 0, total: 4 });
  expect(isFirst(w)).toBe(true);
  expect(back(w)).toEqual(w);
  w = next(next(next(w)));
  expect(isLast(w)).toBe(true);
  expect(next(w)).toEqual(w);
  expect(back(w).index).toBe(2);
});

it("with no permissions to request, the welcome is the last", () => {
  const w = start({ ...VIEW, permissions: [] });
  expect(isFirst(w) && isLast(w)).toBe(true);
});

it("explains each permission", () => {
  for (const p of VIEW.permissions) {
    const info = PERMISSION_INFO[p];
    expect(info.title).not.toBe("");
    expect(info.why).not.toBe("");
    expect(info.how).not.toBe("");
  }
});
