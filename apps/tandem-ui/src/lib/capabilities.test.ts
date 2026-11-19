// T-244: the demo repeats what the agent announces (which modules have a switch, what each system
// says about the administrator service). These tests read the agent's source so the copy cannot drift,
// and check each pair of systems, since the demo is what the screenshots and acceptance runs use.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { hasInputPermissions, isSwitchable, PENDING_SWITCHABLE, perUserAdminNote, SWITCHABLE_MODULES } from "./capabilities";
import { demoDesk, demoOptions, DemoSource } from "./store/demo";
import type { DemoPair } from "./store/demo";

const REPO = join(__dirname, "../../../..");
const MODULES_RS = readFileSync(join(REPO, "crates/tandemd/src/modules.rs"), "utf8");
const MSG_RS = readFileSync(join(REPO, "crates/tandem-proto/src/msg.rs"), "utf8");

/** The ids of the modules whose `impl Module` says `fn switchable` -> `true` in the agent. */
function agentSwitchable(): string[] {
  const idOf = new Map<string, string>();
  for (const m of MSG_RS.matchAll(/ModuleId::(\w+) => "(\w+)",/g)) idOf.set(m[1], m[2]);
  const out: string[] = [];
  // Only the real modules (before the tests module, whose `Fake` has a configurable switch).
  const prod = MODULES_RS.split("#[cfg(test)]")[0];
  for (const block of prod.split(/^impl Module for /m).slice(1)) {
    const id = block.match(/ModuleId::(\w+)/)?.[1];
    const sw = block.match(/fn switchable\(&self\) -> bool \{\s*(true|false)/)?.[1];
    if (id && sw === "true") out.push(idOf.get(id) ?? id);
  }
  return out.sort();
}

describe("capabilities shared with the agent", () => {
  it("the switchable modules are exactly the ones the agent can turn off", () => {
    const agent = agentSwitchable();
    // TODO(T-246): the agent will also switch Tunnels; until `modules.rs` says so it is only
    // pending, and it is left out of this assertion (nothing else is).
    const known = agent.filter((id) => !PENDING_SWITCHABLE.includes(id));
    expect(known).toEqual(["clipboard", "input"]);
    expect([...SWITCHABLE_MODULES].sort()).toEqual(known);
    // Once the agent announces a pending module, it must move to SWITCHABLE_MODULES (this fails otherwise).
    for (const id of agent) expect(SWITCHABLE_MODULES.includes(id) || PENDING_SWITCHABLE.includes(id), id).toBe(true);
  });

  it("only the pending module is switchable beyond the agent's list", () => {
    expect(PENDING_SWITCHABLE).toEqual(["tunnels"]);
    expect(isSwitchable("monitors")).toBe(false);
    expect(isSwitchable("secure_desktop")).toBe(false);
    expect(isSwitchable("power")).toBe(false);
  });

  it("only macOS asks for input permissions", () => {
    expect(hasInputPermissions("macos")).toBe(true);
    expect(hasInputPermissions("windows")).toBe(false);
  });

  it("the administrator service note follows the system and names the other one's words nowhere", () => {
    expect(perUserAdminNote("macos")).toContain("Move Tandem to Applications");
    expect(perUserAdminNote("macos")).not.toContain("Program Files");
    expect(perUserAdminNote("windows")).toContain("Program Files");
    expect(perUserAdminNote("windows")).not.toContain("Applications");
  });
});

const PAIRS: DemoPair[] = ["mac-win", "win-win", "mac-mac"];

describe.each(PAIRS)("demo modules in the %s pair", (pair) => {
  for (const here of ["a", "b"] as const) {
    it(`machine ${here}: only switchable modules have a switch (Tunnels pending T-246)`, () => {
      const d = demoDesk(demoOptions(`demo=${pair}&still&here=${here}`));
      for (const m of ["a", "b"] as const) {
        const mods = d.machines[m]?.modules ?? [];
        expect(mods.length).toBeGreaterThan(0);
        for (const mod of mods) expect(mod.switchable, `${m}/${mod.id}`).toBe(isSwitchable(mod.id));
        expect(mods.filter((x) => x.switchable).map((x) => x.id).sort()).toEqual(["clipboard", "input", "tunnels"]);
      }
    });

    it(`machine ${here}: the administrator service note uses its own system's wording`, async () => {
      const src = new DemoSource(demoOptions(`demo=${pair}&still&here=${here}&admin=off`));
      const d = demoDesk(demoOptions(`demo=${pair}&still&here=${here}&admin=off`));
      await src.start(() => {});
      const os = d.machines[here]?.os;
      const note = (await src.adminStatus()).note;
      expect(note).toBe(perUserAdminNote(os ?? "windows"));
      if (os === "macos") expect(note).not.toMatch(/Program Files|Windows/);
      else expect(note).not.toMatch(/Applications|macOS/);
    });
  }
});
