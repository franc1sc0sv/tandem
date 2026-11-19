// G2 (T-170): los textos del demo salen del sistema de cada máquina, no de un Mac/PC fijo.
import { describe, expect, it } from "vitest";

import { type DemoPair, demoDesk, demoOptions } from "./demo";

const WIN = /WSL|PowerShell|Windows/;

describe.each(["mac-win", "win-win", "mac-mac"] as DemoPair[])("demo %s", (pair) => {
  const d = demoDesk(demoOptions(`demo=${pair}&still`));
  const desk = { ...d, machines: { a: d.machines.a!, b: d.machines.b! } };
  const oses = [desk.machines.a.os, desk.machines.b.os];
  it("ningún túnel menciona un sistema que su máquina destino no tiene", () => {
    for (const t of desk.tunnels) {
      if (desk.machines[t.to].os === "macos") expect(t.env).not.toMatch(WIN);
      else expect(t.env).not.toBe("zsh");
    }
  });
  it("títulos y módulos no nombran un sistema ajeno ni cortan el nombre", () => {
    if (!oses.includes("windows")) for (const t of desk.tunnels) expect(t.title).not.toMatch(/Windows/);
    if (!oses.includes("macos")) for (const t of desk.tunnels) expect(t.title).not.toMatch(/macOS/);
    for (const m of [desk.machines.a, desk.machines.b]) {
      const mon = m.modules.find((x) => x.id === "monitors");
      if (m.os !== "macos") continue;
      expect(mon?.detail).not.toMatch(/muestra la Mac\./);
      if (!m.name.startsWith("MacBook")) expect(mon?.detail).toContain(`shows the ${m.name}.`);
    }
  });
});
