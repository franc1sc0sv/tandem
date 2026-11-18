import { PNG } from "pngjs";
import { describe, expect, it } from "vitest";

import { SCREEN_GATE, SHOTS, TUNELES, SIDEBAR_GATE, appUrl, crop, mask } from "./shots-spec.ts";

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
      expect(s.name).toMatch(/^[a-z-]+$/);
      expect(s.query).toContain("still");
      expect(s.page === "tray.html" ? [s.width, s.height] : [s.width, s.height]).toEqual(s.page === "tray.html" ? [380, 600] : [1280, 860]);
    }
  });

  it("hay captura del demo con dos Windows y con dos Mac (D2)", () => {
    expect(SHOTS.find((s) => s.name === "puesto-win-win")?.query).toContain("demo=win-win");
    expect(SHOTS.find((s) => s.name === "puesto-mac-mac")?.query).toContain("demo=mac-mac");
  });

  it("la barra lateral de V3-Puesto tiene el umbral del criterio (≥ 0.99, peor cuadro ≥ 0.97)", () => {
    const puesto = SHOTS.find((s) => s.name === "puesto");
    expect(puesto?.mockup).toBe("V3-Puesto");
    expect(puesto?.regions?.[0]?.min).toEqual(SIDEBAR_GATE);
    // T-113: y toda la pantalla.
    expect(puesto?.min).toEqual(SIDEBAR_GATE);
    expect(SIDEBAR_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    // Lo único que se tapa es lo que D1 quita.
    expect(puesto?.ignore?.every((i) => i.reason.startsWith("D1"))).toBe(true);
  });

  it("la página Máquina (T-115) tiene umbral en la barra lateral y en el contenido", () => {
    const b = SHOTS.find((s) => s.name === "maquina-b");
    expect(b?.mockup).toBe("V3-Maquina");
    expect(b?.regions?.map((r) => [r.name, r.min])).toEqual([
      ["barra lateral", SIDEBAR_GATE],
      ["contenido", SIDEBAR_GATE],
    ]);
    const a = SHOTS.find((s) => s.name === "maquina-a");
    expect(a?.mockup).toBe("F-MaquinaMac");
    // El estado que dibuja el mockup (falta un permiso) y solo el contenido, con umbral.
    expect(a?.query).toContain("trouble=permissions");
    expect(a?.content?.min).toEqual(SIDEBAR_GATE);
    expect(a?.content?.appWidth).toBe(1492);
    expect(a?.content?.masks.map((m) => m.reason.split(":")[0])).toEqual(["D17", "G4"]);
    // Nada tapado en V3-Maquina más allá de D1.
    expect(SHOTS.find((s) => s.name === "maquina-b")?.ignore?.every((i) => i.reason.startsWith("D1"))).toBe(true);
  });

  it("recorta la zona pedida", () => {
    const out = PNG.sync.read(crop(solid(6, 5, [9, 9, 9]), { x: 1, y: 2, width: 3, height: 9 }));
    expect([out.width, out.height]).toEqual([3, 3]);
  });

  it("la pantalla Red entera exige el umbral y solo tapa lo permitido por mockups/README.md", () => {
    const red = SHOTS.find((s) => s.name === "red");
    expect(red?.mockup).toBe("V3-Red");
    expect(red?.min).toEqual(SCREEN_GATE);
    expect(SCREEN_GATE).toEqual({ similarity: 0.99, worstTile: 0.97 });
    expect(red?.ignore?.map((i) => i.reason.split(":")[0])).toEqual(["D1", "T-102"]);
    // Una sola zona de la pantalla se tapa además de D1: el rótulo de la ventana de la gráfica.
    expect(red?.ignore?.[1]?.mockupSelector).toContain("Latencia · 10 min");
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

describe("Túneles (T-114)", () => {
  it("compara los seis estados del mockup y el plegado con el umbral de pantalla, con los clics del mockup", () => {
    const gated = TUNELES.filter((s) => s.min);
    expect(gated.map((s) => s.state ?? "inicial")).toEqual(["inicial", "proceso-2", "tunel-2-procesos", "tunel-sin-procesos", "tunel-lora", "tunel-cerrado", "proyecto-plegado"]);
    for (const s of gated) expect(s.min).toEqual(SCREEN_GATE);
    for (const s of TUNELES) expect(SHOTS).toContain(s);
  });

  it("los estados que el mockup no dibuja (vacío y 12 procesos) no llevan umbral de píxeles", () => {
    const extra = TUNELES.filter((s) => !s.min);
    expect(extra.map((s) => s.query)).toEqual(["demo&still&tunnels=none", "demo&still&tunnels=many"]);
  });
});
