import { describe, expect, it } from "vitest";

import { modelOf, setupLines, swapWiring, wiringText } from "./desk";

describe("puesto", () => {
  it("describe el cableado con la PC primero", () => {
    expect(wiringText({ mac: "hdmi", pc: "displayport" })).toBe("DP → PC · HDMI → Mac");
    expect(wiringText({ mac: "displayport" })).toBe("DP → Mac");
    expect(wiringText({})).toBe("sin cablear");
  });

  it("invierte el cableado con un clic", () => {
    expect(swapWiring({ pc: "displayport", mac: "hdmi" })).toEqual({ pc: "hdmi", mac: "displayport" });
    expect(swapWiring(swapWiring({ pc: "displayport", mac: "hdmi" }))).toEqual({ pc: "displayport", mac: "hdmi" });
    // Cableado a una sola máquina: cambia de entrada.
    expect(swapWiring({ pc: "hdmi" })).toEqual({ pc: "displayport" });
  });

  it("cuenta si la configuración la generó el agente", () => {
    expect(setupLines("mac", { config_file: "/x/tandem.toml", generated: true, port: 52000 })).toEqual([
      "Esta máquina: la Mac.",
      "Tandem detectó los monitores y escribió la configuración solo: no hace falta tocar ningún archivo.",
      "El agente escucha en el puerto UDP 52000.",
    ]);
    expect(setupLines("pc", { config_file: null, generated: false, port: null })).toEqual(["Esta máquina: la PC."]);
    expect(setupLines("pc", null)).toEqual(["Esta máquina: la PC."]);
  });

  it("muestra el modelo si dice algo más que la etiqueta", () => {
    expect(modelOf({ id: "sam711a", label: '34"', edid_model: "SAM711A", wiring: {} })).toBe("SAM711A");
    expect(modelOf({ id: "x", label: "DELL", edid_model: "DELL", wiring: {} })).toBeNull();
    expect(modelOf({ id: "x", label: "a", wiring: {} })).toBeNull();
  });
});
