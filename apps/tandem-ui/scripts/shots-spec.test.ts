import { PNG } from "pngjs";
import { describe, expect, it } from "vitest";

import { SCREEN_GATE, SHOTS, TUNNELS, SIDEBAR_GATE, appUrl, crop, mask } from "./shots-spec.ts";

function solid(w: number, h: number, rgb: [number, number, number]): Buffer {
  const p = new PNG({ width: w, height: h });
  for (let i = 0; i < w * h; i++) p.data.set([...rgb, 255], i * 4);
  return PNG.sync.write(p);
}

describe("screenshot harness", () => {
  it("each screen has a unique name, a mockup and the mockup size", () => {
    const names = SHOTS.map((s) => s.name);
    expect(new Set(names).size).toBe(names.length);
    for (const s of SHOTS) {
      expect(s.query).toContain("demo");
      expect(s.name).toMatch(/^[a-z-]+$/);
      expect(s.query).toContain("still");
      expect(s.page === "tray.html" ? [s.width, s.height] : [s.width, s.height]).toEqual(s.page === "tray.html" ? [380, 600] : [1280, 860]);
    }
  });

  it("there is a demo screenshot with two Windows and with two Macs (D2)", () => {
    expect(SHOTS.find((s) => s.name === "desk-win-win")?.query).toContain("demo=win-win");
    expect(SHOTS.find((s) => s.name === "desk-mac-mac")?.query).toContain("demo=mac-mac");
  });

  it("the V3-Desk sidebar has the criterion threshold (≥ 0.99, worst tile ≥ 0.97)", () => {
    const desk = SHOTS.find((s) => s.name === "desk");
    expect(desk?.mockup).toBe("V3-Desk");
    expect(desk?.regions?.[0]?.min).toEqual(SIDEBAR_GATE);
    // T-113: and the whole screen.
    expect(desk?.min).toEqual(SIDEBAR_GATE);
    expect(SIDEBAR_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    // The only thing masked is what D1 removes.
    expect(desk?.ignore?.every((i) => i.reason.startsWith("D1"))).toBe(true);
  });

  it("the Machine page (T-115) has a threshold on the sidebar and on the content", () => {
    const b = SHOTS.find((s) => s.name === "machine-b");
    expect(b?.mockup).toBe("V3-Machine");
    expect(b?.regions?.map((r) => [r.name, r.min])).toEqual([
      ["sidebar", SIDEBAR_GATE],
      ["content", SIDEBAR_GATE],
    ]);
    const a = SHOTS.find((s) => s.name === "machine-a");
    expect(a?.mockup).toBe("F-MachineMac");
    // The state the mockup draws (one permission missing) and only the content, with a threshold.
    expect(a?.query).toContain("trouble=permissions");
    expect(a?.content?.min).toEqual(SIDEBAR_GATE);
    expect(a?.content?.appWidth).toBe(1492);
    expect(a?.content?.masks.map((m) => m.reason.split(":")[0])).toEqual(["D17", "G4"]);
    // Nothing masked in V3-Machine beyond D1 and the Tunnels switch the agent cannot flip (T-244).
    expect(SHOTS.find((s) => s.name === "machine-b")?.ignore?.map((i) => i.reason.split(":")[0])).toEqual(["D1", "T-244"]);
  });

  it("crops the requested region", () => {
    const out = PNG.sync.read(crop(solid(6, 5, [9, 9, 9]), { x: 1, y: 2, width: 3, height: 9 }));
    expect([out.width, out.height]).toEqual([3, 3]);
  });

  it("the whole Network screen requires the threshold and only masks what mockups/README.md allows", () => {
    const network = SHOTS.find((s) => s.name === "network");
    expect(network?.mockup).toBe("V3-Network");
    expect(network?.min).toEqual(SCREEN_GATE);
    expect(SCREEN_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    expect(network?.ignore?.map((i) => i.reason.split(":")[0])).toEqual(["D1", "T-102"]);
    // Only one region of the screen is masked besides D1: the label of the chart window.
    expect(network?.ignore?.[1]?.mockupSelector).toContain("Latency · 10 min");
  });

  it("masks the requested regions and nothing else", () => {
    const out = PNG.sync.read(mask(solid(4, 4, [0, 0, 0]), [{ x: 1, y: 1, width: 2, height: 1 }]));
    const px = (x: number, y: number) => [...out.data.slice((y * 4 + x) * 4, (y * 4 + x) * 4 + 3)];
    expect(px(1, 1)).toEqual([255, 0, 255]);
    expect(px(2, 1)).toEqual([255, 0, 255]);
    expect(px(0, 0)).toEqual([0, 0, 0]);
    expect(px(1, 2)).toEqual([0, 0, 0]);
  });

  it("builds the app URL", () => {
    expect(appUrl("http://app.local", { page: "index.html", query: "demo&still", hash: "#/network" })).toBe("http://app.local/index.html?demo&still#/network");
    expect(appUrl("http://app.local", { page: "tray.html", query: "", hash: "" })).toBe("http://app.local/tray.html");
  });
});

describe("Tunnels (T-114)", () => {
  it("compares the six mockup states and the collapsed one with the screen threshold, with the mockup clicks", () => {
    const gated = TUNNELS.filter((s) => s.min);
    expect(gated.map((s) => s.state ?? "initial")).toEqual(["initial", "process-2", "tunnel-2-processes", "tunnel-no-processes", "tunnel-lora", "tunnel-closed", "project-collapsed"]);
    for (const s of gated) expect(s.min).toEqual(SCREEN_GATE);
    for (const s of TUNNELS) expect(SHOTS).toContain(s);
  });

  it("the states the mockup does not draw (empty and 12 processes) have no pixel threshold", () => {
    const extra = TUNNELS.filter((s) => !s.min);
    expect(extra.map((s) => s.query)).toEqual(["demo&still&tunnels=none", "demo&still&tunnels=many"]);
  });
});
