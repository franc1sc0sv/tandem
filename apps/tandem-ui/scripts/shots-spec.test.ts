import { PNG } from "pngjs";
import { describe, expect, it } from "vitest";

import { SHOTS, SIDEBAR_GATE, appUrl, mask } from "./shots-spec.ts";

function solid(w: number, h: number, rgb: [number, number, number]): Buffer {
  const p = new PNG({ width: w, height: h });
  for (let i = 0; i < w * h; i++) p.data.set([...rgb, 255], i * 4);
  return PNG.sync.write(p);
}

describe("arnés de capturas", () => {
  it("cada pantalla tiene nombre único, mockup y tamaño del mockup", () => {
    const names = SHOTS.map((s) => s.name);
    expect(new Set(names).size).toBe(names.length);
    for (const s of SHOTS) {
      expect(s.query).toContain("demo");
      expect(s.query).toContain("still");
      expect(s.page === "tray.html" ? [s.width, s.height] : [s.width, s.height]).toEqual(s.page === "tray.html" ? [380, 600] : [1280, 860]);
    }
  });

  it("la barra lateral de V3-Puesto tiene el umbral del criterio (≥ 0.99, peor cuadro ≥ 0.97)", () => {
    const puesto = SHOTS.find((s) => s.name === "puesto");
    expect(puesto?.mockup).toBe("V3-Puesto");
    expect(puesto?.regions?.[0]?.min).toEqual(SIDEBAR_GATE);
    expect(SIDEBAR_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    // Lo único que se tapa es lo que D1 quita.
    expect(puesto?.ignore?.every((i) => i.reason.startsWith("D1"))).toBe(true);
  });

  it("tapa las zonas pedidas y nada más", () => {
    const out = PNG.sync.read(mask(solid(4, 4, [0, 0, 0]), [{ x: 1, y: 1, width: 2, height: 1 }]));
    const px = (x: number, y: number) => [...out.data.slice((y * 4 + x) * 4, (y * 4 + x) * 4 + 3)];
    expect(px(1, 1)).toEqual([255, 0, 255]);
    expect(px(2, 1)).toEqual([255, 0, 255]);
    expect(px(0, 0)).toEqual([0, 0, 0]);
    expect(px(1, 2)).toEqual([0, 0, 0]);
  });

  it("arma la URL de la app", () => {
    expect(appUrl("http://app.local", { page: "index.html", query: "demo&still", hash: "#/red" })).toBe("http://app.local/index.html?demo&still#/red");
    expect(appUrl("http://app.local", { page: "tray.html", query: "", hash: "" })).toBe("http://app.local/tray.html");
  });
});
