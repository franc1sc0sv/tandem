// T-244: the demo repeats what the agent announces (which modules have a switch, what each system
// says about the administrator service). These tests read the agent's source so the copy cannot drift,
// and check each pair of systems, since the demo is what the screenshots and acceptance runs use.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { hasInputPermissions, isSwitchable, perUserAdminNote, SWITCHABLE_MODULES } from "./capabilities";
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
    expect(agentSwitchable()).toEqual(["clipboard", "input"]);
    expect([...SWITCHABLE_MODULES].sort()).toEqual(agentSwitchable());
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
    it(`machine ${here}: only the agent's switchable modules have a switch`, () => {
      const d = demoDesk(demoOptions(`demo=${pair}&still&here=${here}`));
      for (const m of ["a", "b"] as const) {
        const mods = d.machines[m]?.modules ?? [];
        expect(mods.length).toBeGreaterThan(0);
        for (const mod of mods) expect(mod.switchable, `${m}/${mod.id}`).toBe(isSwitchable(mod.id));
        expect(mods.filter((x) => x.switchable).map((x) => x.id).sort()).toEqual(["clipboard", "input"]);
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
