import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { ARRANGEMENT, SCREEN_GATE, SHOTS } from "./shots-spec.ts";

describe("arrangement and crossing test screenshots (T-117)", () => {
  it("cover the 4 sides × 3 alignments, each chosen screen and the test states, with the ACCEPTANCE threshold", () => {
    expect(SCREEN_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    for (const s of ARRANGEMENT) expect(SHOTS).toContain(s);
    const states = ARRANGEMENT.filter((s) => s.mockup === "V3-Arrangement").map((s) => s.state ?? "");
    for (const side of ["top", "bottom", "left", "right"]) for (const al of ["start", "center", "end"]) expect(states).toContain(`side-${side}-${al}`);
    expect(states).toEqual(expect.arrayContaining(["", "screen-mac", "screen-unsupported"]));
    const crossing = ARRANGEMENT.filter((s) => s.mockup === "F-CrossingTest").map((s) => s.state ?? "");
    expect(crossing).toEqual(expect.arrayContaining(["", "crossed", "no-cross"]));
    // Everything with its own mockup requires the threshold; the wrong edge has no mockup.
    for (const s of ARRANGEMENT) expect(s.min, s.name).toEqual(s.name === "crossing-other-edge" ? undefined : SCREEN_GATE);
  });

  it("the clicks of each state are those of the mockup (`tools/mockup-render/src/states.ts`)", () => {
    const states = readFileSync(join(process.cwd(), "..", "..", "tools", "mockup-render", "src", "states.ts"), "utf8");
    expect(states).toContain("click(`.slot[aria-label=\"${label}\"]`), click(`.seg .sb >> nth=${i}`)");
    expect(states).toContain(`click('.scr:has-text("MacBook Pro")')`);
    expect(states).toContain(`click('.scr:has-text("Odyssey G5 32")')`);
  });

  it("the only things masked are documented in mockups/README.md", () => {
    const readme = readFileSync(join(process.cwd(), "..", "..", "mockups", "README.md"), "utf8");
    for (const s of ARRANGEMENT) {
      for (const i of s.ignore ?? []) {
        if (i.reason.startsWith("D1")) expect(readme).toContain("Add machine");
        else expect(readme).toContain("Simulate…");
      }
    }
  });
});
