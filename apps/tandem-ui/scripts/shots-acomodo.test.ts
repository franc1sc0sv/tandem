import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { ACOMODO, SCREEN_GATE, SHOTS } from "./shots-spec.ts";

describe("capturas del acomodo y la prueba de cruce (T-117)", () => {
  it("cubren los 4 lados × 3 alineaciones, cada pantalla elegida y los estados de la prueba, con el umbral de ACCEPTANCE", () => {
    expect(SCREEN_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    for (const s of ACOMODO) expect(SHOTS).toContain(s);
    const states = ACOMODO.filter((s) => s.mockup === "V3-Arrangement").map((s) => s.state ?? "");
    for (const side of ["top", "bottom", "left", "right"]) for (const al of ["start", "center", "end"]) expect(states).toContain(`side-${side}-${al}`);
    expect(states).toEqual(expect.arrayContaining(["", "screen-mac", "screen-unsupported"]));
    const cruce = ACOMODO.filter((s) => s.mockup === "F-CrossingTest").map((s) => s.state ?? "");
    expect(cruce).toEqual(expect.arrayContaining(["", "crossed", "no-cross"]));
    // Todo lo que tiene mockup propio exige el umbral; el borde equivocado no tiene mockup.
    for (const s of ACOMODO) expect(s.min, s.name).toEqual(s.name === "crossing-other-edge" ? undefined : SCREEN_GATE);
  });

  it("los clics de cada estado son los del mockup (`tools/mockup-render/src/states.ts`)", () => {
    const states = readFileSync(join(process.cwd(), "..", "..", "tools", "mockup-render", "src", "states.ts"), "utf8");
    expect(states).toContain("click(`.slot[aria-label=\"${label}\"]`), click(`.seg .sb >> nth=${i}`)");
    expect(states).toContain(`click('.scr:has-text("MacBook Pro")')`);
    expect(states).toContain(`click('.scr:has-text("Odyssey G5 32")')`);
  });

  it("lo único que se tapa está documentado en mockups/README.md", () => {
    const readme = readFileSync(join(process.cwd(), "..", "..", "mockups", "README.md"), "utf8");
    for (const s of ACOMODO) {
      for (const i of s.ignore ?? []) {
        if (i.reason.startsWith("D1")) expect(readme).toContain("Add machine");
        else expect(readme).toContain("Simulate…");
      }
    }
  });
});
