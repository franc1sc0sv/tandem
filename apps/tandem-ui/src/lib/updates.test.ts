import { describe, expect, it } from "vitest";
import { availableRow, checkLabel, upToDate, versionRow, type UpdateView } from "./updates";

const base: UpdateView = { here: "0.3.0", peer: "0.3.0", paired: true, available: null, checking: false, step: null, error: null, auto: true };

describe("Actualizaciones", () => {
  it("dice si las dos máquinas están en la misma versión", () => {
    expect(versionRow(base)).toEqual({ title: "Tandem 0.3.0", detail: "Todas las máquinas del puesto en la misma versión." });
    expect(versionRow({ ...base, peer: "0.3.1" }).detail).toBe("La otra máquina tiene la 0.3.1.");
    expect(versionRow({ ...base, peer: null }).detail).toMatch(/Todavía no se sabe/);
    expect(versionRow({ ...base, paired: false, peer: null }).detail).toBe("Esta máquina trabaja sola.");
  });

  it("el botón cambia de «Buscar ahora» a «Buscar de nuevo»", () => {
    expect(checkLabel(base, false)).toBe("Buscar ahora");
    expect(checkLabel(base, true)).toBe("Buscar de nuevo");
    expect(checkLabel({ ...base, checking: true }, true)).toBe("Buscando…");
  });

  it("la versión nueva dice cómo se instala y en qué paso va", () => {
    expect(availableRow(base)).toBeNull();
    const found = { ...base, available: "0.3.1" };
    expect(availableRow(found)).toEqual({
      title: "Hay una versión nueva: 0.3.1",
      detail: "Se instala en todas las máquinas, una a la vez, sin cortar el teclado.",
    });
    expect(availableRow({ ...found, paired: false })?.detail).toBe("Se instala en esta máquina.");
    expect(availableRow({ ...found, step: "wait_peer" })?.detail).toMatch(/esta sigue con el teclado/);
  });

  it("al día solo después de buscar y sin errores", () => {
    expect(upToDate(base, false)).toBe(false);
    expect(upToDate(base, true)).toBe(true);
    expect(upToDate({ ...base, error: "sin red" }, true)).toBe(false);
    expect(upToDate({ ...base, available: "0.3.1" }, true)).toBe(false);
  });
});
