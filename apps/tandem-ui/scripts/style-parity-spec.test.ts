import { describe, expect, it } from "vitest";

import {
  ALL,
  GROUPS,
  PAIRS,
  animations,
  compareStyles,
  firstFamily,
  formatReport,
  normalizeCurve,
  normalizeNumbers,
  propertiesOf,
  splitList,
  toMs,
  transitions,
  type Styles,
} from "./style-parity-spec.ts";

// Estilos calculados como los da Chromium para el botón de 38 px de V3-Acomodo.
const BTN: Styles = {
  height: "38px",
  "padding-top": "0px",
  "padding-right": "15px",
  "padding-bottom": "0px",
  "padding-left": "15px",
  "font-family": 'Inter, system-ui, sans-serif',
  "font-size": "13px",
  "font-weight": "500",
  color: "rgb(232, 232, 235)",
  "border-top-left-radius": "10px",
  "border-top-right-radius": "10px",
  "border-bottom-right-radius": "10px",
  "border-bottom-left-radius": "10px",
  ...Object.fromEntries(["top", "right", "bottom", "left"].flatMap((s) => [[`border-${s}-width`, "1px"], [`border-${s}-style`, "solid"], [`border-${s}-color`, "rgb(46, 49, 56)"]])),
  "background-color": "rgba(0, 0, 0, 0)",
  "background-image": "linear-gradient(rgb(31, 32, 37), rgb(24, 25, 29))",
  "box-shadow": "rgba(255, 255, 255, 0.05) 0px 1px 0px 0px inset, rgba(0, 0, 0, 0.3) 0px 1px 2px 0px",
  "transition-property": "transform, border-color, box-shadow",
  "transition-duration": "0.18s, 0.18s, 0.18s",
  "transition-timing-function": "cubic-bezier(0.2, 0.8, 0.2, 1), ease, ease",
  "transition-delay": "0s, 0s, 0s",
  "animation-name": "none",
  "animation-duration": "0s",
  "animation-timing-function": "ease",
  "animation-iteration-count": "1",
  "animation-fill-mode": "none",
};

const with_ = (over: Styles): Styles => ({ ...BTN, ...over });

describe("normalización", () => {
  it("separa listas solo por las comas de primer nivel", () => {
    expect(splitList("transform, border-color")).toEqual(["transform", "border-color"]);
    expect(splitList("cubic-bezier(0.2, 0.8, 0.2, 1), ease")).toEqual(["cubic-bezier(0.2, 0.8, 0.2, 1)", "ease"]);
    expect(splitList("rgba(0, 0, 0, 0.3) 0px 1px 2px 0px")).toEqual(["rgba(0, 0, 0, 0.3) 0px 1px 2px 0px"]);
    expect(splitList("")).toEqual([]);
  });

  it("pasa los tiempos a milisegundos", () => {
    expect(toMs("0.18s")).toBe(180);
    expect(toMs("180ms")).toBe(180);
    expect(toMs("0s")).toBe(0);
  });

  it("las curvas con nombre equivalen a su cubic-bezier", () => {
    expect(normalizeCurve("ease")).toBe(normalizeCurve("cubic-bezier(0.25, 0.1, 0.25, 1)"));
    expect(normalizeCurve("linear")).toBe(normalizeCurve("cubic-bezier(0,0,1,1)".replace(/,/g, ", ")));
    expect(normalizeCurve("ease-out")).not.toBe(normalizeCurve("ease"));
  });

  it("redondea los restos de cálculo y no confunde valores distintos", () => {
    expect(normalizeNumbers("37.9999px")).toBe("38px");
    expect(normalizeNumbers("-0.001px")).toBe("0px");
    expect(normalizeNumbers("rgba(0,  0, 0, 0.3)")).toBe("rgba(0, 0, 0, 0.3)");
    expect(normalizeNumbers("12.5px")).not.toBe(normalizeNumbers("13px"));
  });

  it("de la familia de fuente cuenta la primera", () => {
    expect(firstFamily('"Inter", system-ui, sans-serif')).toBe("inter");
    expect(firstFamily("Inter, -apple-system, sans-serif")).toBe("inter");
    expect(firstFamily('"JetBrains Mono", monospace')).not.toBe("inter");
  });

  it("arma las transiciones por propiedad, repitiendo las listas cortas y sin las nulas", () => {
    const t = transitions({ "transition-property": "transform, border-color", "transition-duration": "0.18s", "transition-timing-function": "ease", "transition-delay": "0s" });
    expect([...t.keys()]).toEqual(["transform", "border-color"]);
    expect(t.get("border-color")).toBe(t.get("transform"));
    // El valor inicial (`all 0s ease 0s`) no es una transición.
    expect(transitions({ "transition-property": "all", "transition-duration": "0s", "transition-timing-function": "ease", "transition-delay": "0s" }).size).toBe(0);
    // El retraso cuenta.
    expect(transitions({ "transition-property": "left", "transition-duration": "0s", "transition-timing-function": "ease", "transition-delay": "0.1s" }).get("left")).toContain("+100ms");
  });

  it("de las animaciones mira duración, curva, repeticiones y relleno, no el nombre ni el retraso", () => {
    const a = animations({ "animation-name": "svelte-1x2y3z-card-up", "animation-duration": "0.45s", "animation-timing-function": "cubic-bezier(0.2, 0.8, 0.2, 1)", "animation-iteration-count": "1", "animation-fill-mode": "both" });
    const m = animations({ "animation-name": "up", "animation-duration": "450ms", "animation-timing-function": "cubic-bezier(.2,.8,.2,1)".replace(/,/g, ", "), "animation-iteration-count": "1", "animation-fill-mode": "both" });
    expect(a).toEqual(m);
    expect(animations({ "animation-name": "none" })).toEqual([]);
  });
});

describe("compareStyles", () => {
  it("dos elementos iguales no dan diferencias", () => {
    expect(compareStyles("botón", BTN, { ...BTN }, ALL)).toEqual([]);
  });

  it("detecta un alto, un relleno o un radio distinto y dice cuál", () => {
    const d = compareStyles("botón", with_({ height: "36px", "padding-left": "14px", "border-top-left-radius": "9px" }), BTN, ALL);
    expect(d.map((x) => [x.group, x.property, x.app, x.mockup])).toEqual([
      ["alto", "height", "36px", "38px"],
      ["relleno", "padding-left", "14px", "15px"],
      ["radio", "border-top-left-radius", "9px", "10px"],
    ]);
  });

  it("detecta fuente, fondo, borde y sombra distintos", () => {
    const d = compareStyles(
      "botón",
      with_({ "font-weight": "600", "background-image": "none", "border-top-color": "rgb(49, 52, 59)", "box-shadow": "none" }),
      BTN,
      ALL,
    );
    expect(d.map((x) => x.property)).toEqual(["font-weight", "border-top-color", "background-image", "box-shadow"]);
  });

  it("una transición de más, de menos o con otra curva o duración es una diferencia", () => {
    const extra = compareStyles("botón", with_({ "transition-property": "transform, border-color, box-shadow, opacity", "transition-duration": "0.18s, 0.18s, 0.18s, 0.3s", "transition-timing-function": "cubic-bezier(0.2, 0.8, 0.2, 1), ease, ease, ease" }), BTN, ["transition"]);
    expect(extra).toEqual([{ pair: "botón", group: "transition", property: "transition: opacity", app: "300ms cubic-bezier(0.25, 0.1, 0.25, 1)", mockup: "sin transición" }]);

    const curve = compareStyles("botón", with_({ "transition-timing-function": "ease, ease, ease" }), BTN, ["transition"]);
    expect(curve.map((x) => x.property)).toEqual(["transition: transform"]);

    const dur = compareStyles("botón", with_({ "transition-duration": "0.2s, 0.18s, 0.18s" }), BTN, ["transition"]);
    expect(dur[0]?.app).toMatch(/^200ms/);
    expect(dur[0]?.mockup).toMatch(/^180ms/);

    const none = compareStyles("botón", with_({ "transition-property": "all", "transition-duration": "0s" }), BTN, ["transition"]);
    expect(none.map((x) => x.app)).toEqual(["sin transición", "sin transición", "sin transición"]);
  });

  it("una animación que falta o con otra duración es una diferencia", () => {
    const card: Styles = { "animation-name": "up", "animation-duration": "0.45s", "animation-timing-function": "cubic-bezier(0.2, 0.8, 0.2, 1)", "animation-iteration-count": "1", "animation-fill-mode": "both" };
    expect(compareStyles("tarjeta", { ...card, "animation-name": "none" }, card, ["animation"])[0]?.app).toBe("ninguna");
    expect(compareStyles("tarjeta", { ...card, "animation-duration": "0.3s" }, card, ["animation"])).toHaveLength(1);
    expect(compareStyles("tarjeta", { ...card, "animation-iteration-count": "infinite" }, card, ["animation"])).toHaveLength(1);
  });

  it("no compara el color de un borde que no se dibuja", () => {
    const noBorder: Styles = { "border-top-width": "0px", "border-top-style": "none", "border-top-color": "rgb(0, 0, 0)" };
    const d = compareStyles("interruptor", noBorder, { ...noBorder, "border-top-color": "rgb(255, 255, 255)" }, ["borde"]);
    expect(d.filter((x) => x.property === "border-top-color")).toEqual([]);
    // Si en un lado sí se dibuja, el color cuenta (y el ancho también).
    const one = compareStyles("x", { ...noBorder, "border-top-width": "1px", "border-top-style": "solid", "border-top-color": "rgb(46, 49, 56)" }, noBorder, ["borde"]);
    expect(one.map((x) => x.property)).toEqual(["border-top-width", "border-top-style", "border-top-color"]);
  });

  it("solo mira los grupos pedidos", () => {
    expect(compareStyles("tarjeta", with_({ height: "120px" }), BTN, ["relleno", "radio"])).toEqual([]);
  });
});

describe("informe", () => {
  it("dice qué par, qué propiedad y dónde está cada lado", () => {
    const pairs = PAIRS.filter((p) => p.name === "botón peligro" || p.name === "tarjeta");
    const out = formatReport(pairs, [{ pair: "botón peligro", group: "radio", property: "border-top-left-radius", app: "10px", mockup: "9px" }], ["tarjeta: no está en V3-Puesto (div.card)"]);
    const lines = out.split("\n");
    expect(lines[0]).toContain("✗ botón peligro");
    expect(lines[0]).toContain('gallery.html [data-part="btn-danger"] .btn');
    expect(lines[0]).toContain("F-Ajustes button.btn.danger");
    expect(lines[1]).toBe("    radio · border-top-left-radius: app «10px» ≠ mockup «9px»");
    expect(out).toContain("✗ tarjeta");
    expect(out).toContain("! no está en V3-Puesto (div.card)");
  });

  it("marca con ✓ los pares iguales", () => {
    expect(formatReport(PAIRS.slice(0, 1), [])).toBe(`✓ ${PAIRS[0]?.name}`);
  });
});

describe("pares", () => {
  it("cubren los componentes y mockups que pide T-127", () => {
    const has = (mockup: string, sel: RegExp) => PAIRS.some((p) => p.mockup === mockup && sel.test(p.ref.selector));
    expect(has("V3-Acomodo", /^a\.btn\[/)).toBe(true); // botón 38 px
    expect(has("V3-Monitores", /\.btn\.sm/)).toBe(true);
    expect(has("V3-Monitores", /\.btn\.pri\.sm/)).toBe(true);
    expect(has("V3-Monitores", /\.btn\.ghost\.ic\.sm/)).toBe(true);
    expect(has("F-Ajustes", /\.btn\.danger/)).toBe(true);
    expect(has("V3-Acomodo", /\.seg/)).toBe(true);
    expect(has("V3-Monitores", /\.seg/)).toBe(true);
    expect(has("F-Ajustes", /\.sw/)).toBe(true);
    expect(has("V3-Puesto", /\.chip/)).toBe(true);
    expect(has("V3-Puesto", /\.card/)).toBe(true);
  });

  it("tienen nombre único y piden propiedades conocidas", () => {
    const names = PAIRS.map((p) => p.name);
    expect(new Set(names).size).toBe(names.length);
    for (const p of PAIRS) {
      expect(p.groups.length).toBeGreaterThan(0);
      expect(p.app.selector).toMatch(/^\[data-part="[a-z0-9-]+"\] /);
      for (const prop of propertiesOf(p.groups)) expect(Object.values(GROUPS).flat()).toContain(prop);
    }
  });

  it("todos los botones comparan alto, relleno, fuente, radio, borde, fondo, sombra y movimiento", () => {
    for (const p of PAIRS.filter((x) => x.name.startsWith("botón"))) expect(p.groups).toEqual(expect.arrayContaining(ALL));
  });
});
