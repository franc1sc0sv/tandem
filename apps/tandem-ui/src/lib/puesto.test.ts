import { describe, expect, it } from "vitest";

import { demoDesk, demoOptions } from "./store/demo";
import { cursorPoint, DEMO_LOOP_MS, demoPath, flowSeconds, RateMeter, sampleAt, sceneOf } from "./puesto";

const names = { a: "MacBook Pro", b: "PC-Escritorio" };
const colors = { a: "#7aa7e6", b: "#a99be8" };
const desk = () => demoDesk(demoOptions("demo&still"));
const scene = () => sceneOf(desk().layout, names, colors);

describe("escritorios a escala (V3-Puesto)", () => {
  it("el demo cae donde el mockup dibuja la MacBook y el 34\"", () => {
    const s = scene();
    const mac = s.desks.find((d) => d.machine === "a");
    const pc = s.desks.find((d) => d.machine === "b");
    // Medidas del mockup, en % del lienzo (tolerancia de medio punto).
    const near = (v: number | undefined, want: number) => expect(Math.abs((v ?? 99) - want)).toBeLessThan(0.5);
    near(mac?.left, 7.5);
    near(mac?.top, 36);
    near(mac?.width, 22.9);
    near(mac?.height, 35.8);
    near(pc?.left, 32.6);
    near(pc?.top, 10.9);
    near(pc?.width, 60);
    near(pc?.height, 60.9);
    expect(s.aspect).toBeCloseTo(133 / 55, 2);
  });

  it("los nombres y colores salen de cada máquina", () => {
    const s = scene();
    expect(s.desks.map((d) => d.label)).toEqual(["MacBook Pro", '34" · PC-Escritorio']);
    const other = sceneOf(desk().layout, { a: "Estudio", b: "Taller" }, { a: "#5fbfb0", b: "#e6a07a" });
    expect(other.desks.map((d) => d.label)).toEqual(["Estudio", '34" · Taller']);
    expect(other.desks[0]?.wall).not.toBe(s.desks[0]?.wall);
  });

  it("la MacBook tiene base y el monitor, soporte y pie", () => {
    expect(scene().feet).toHaveLength(3);
    expect(scene().feet.filter((f) => f.radius === 6)).toHaveLength(1);
  });

  it("el borde de cruce cae en el lado de B donde queda A y mide lo que comparten", () => {
    const s = scene();
    const pc = s.desks.find((d) => d.machine === "b");
    expect(s.edge?.vertical).toBe(true);
    expect(s.edge?.left).toBeCloseTo(pc?.left ?? 0, 5);
    // Con A abajo del todo (align end) el tramo compartido es el alto de la MacBook.
    expect(s.edge?.length).toBeGreaterThan(30);
    expect(s.edge?.length).toBeLessThan(40);
  });

  it("sin pantallas no hay nada que dibujar", () => {
    const s = sceneOf({ ...desk().layout, screens: [] }, names, colors);
    expect(s.desks).toEqual([]);
    expect(s.edge).toBeNull();
  });
});

describe("cursor sobre el escritorio", () => {
  const layout = () => desk().layout;

  it("el centro de la pantalla nativa cae en el centro de su caja", () => {
    const s = scene();
    const mac = s.desks.find((d) => d.machine === "a");
    const p = cursorPoint(s, layout(), { machine: "a", screen: "mbp", x: 756, y: 491 });
    expect(p?.left).toBeCloseTo((mac?.left ?? 0) + (mac?.width ?? 0) / 2, 5);
    expect(p?.top).toBeCloseTo((mac?.top ?? 0) + (mac?.height ?? 0) / 2, 5);
  });

  it("sin id de pantalla se busca por el punto, y fuera de la pantalla se queda en el borde", () => {
    const s = scene();
    const pc = s.desks.find((d) => d.machine === "b");
    const p = cursorPoint(s, layout(), { machine: "b", screen: "", x: 99999, y: -5 });
    expect(p?.left).toBeCloseTo((pc?.left ?? 0) + (pc?.width ?? 0), 5);
    expect(p?.top).toBeCloseTo(pc?.top ?? 0, 5);
  });

  it("una máquina sin pantallas conocidas no se dibuja", () => {
    const l = { ...layout(), screens: layout().screens.map((x) => ({ ...x, native: undefined })) };
    expect(cursorPoint(scene(), l, { machine: "a", screen: "", x: 1, y: 1 })).toBeNull();
  });

  it("ida y vuelta: del punto a la muestra y de la muestra al punto", () => {
    const s = scene();
    for (const d of s.desks) {
      for (const fx of [0.05, 0.3, 0.5, 0.95]) {
        for (const fy of [0.05, 0.5, 0.95]) {
          const pt = { left: d.left + d.width * fx, top: d.top + d.height * fy };
          const sample = sampleAt(s, layout(), pt);
          expect(sample?.machine).toBe(d.machine);
          const back = sample ? cursorPoint(s, layout(), sample) : null;
          expect(back?.left).toBeCloseTo(pt.left, 6);
          expect(back?.top).toBeCloseTo(pt.top, 6);
        }
      }
    }
  });

  it("entre las dos pantallas no hay muestra", () => {
    expect(sampleAt(scene(), layout(), { left: 31.5, top: 50 })).toBeNull();
  });
});

describe("tráfico del enlace", () => {
  it("cuenta eventos por segundo en la ventana", () => {
    const m = new RateMeter();
    for (let t = 0; t < 1000; t += 100) m.push(t);
    expect(m.rate(999)).toBe(10);
    expect(m.rate(1450)).toBe(5);
    expect(m.rate(5000)).toBe(0);
  });

  it("el pulso va más rápido con más tráfico", () => {
    expect(flowSeconds(0)).toBeGreaterThan(flowSeconds(5));
    expect(flowSeconds(5)).toBeGreaterThan(flowSeconds(30));
    expect(flowSeconds(30)).toBe(1);
    expect(flowSeconds(Number.NaN)).toBe(flowSeconds(0));
  });
});

describe("recorrido del demo (los fotogramas de `cur`)", () => {
  it("parte y termina en el mismo punto y pasa por los del mockup", () => {
    expect(demoPath(0)).toEqual({ left: 16, top: 56 });
    expect(demoPath(1)).toEqual({ left: 16, top: 56 });
    expect(demoPath(0.38)).toEqual({ left: 40, top: 48 });
    expect(demoPath(0.62)).toEqual({ left: 63, top: 30 });
    expect(DEMO_LOOP_MS).toBe(7000);
  });

  it("siempre queda dentro del lienzo", () => {
    for (let i = 0; i <= 200; i++) {
      const p = demoPath(i / 200);
      expect(p.left).toBeGreaterThanOrEqual(0);
      expect(p.left).toBeLessThanOrEqual(100);
      expect(p.top).toBeGreaterThanOrEqual(0);
      expect(p.top).toBeLessThanOrEqual(100);
    }
  });
});
