import { describe, expect, it } from "vitest";

import { actions, advance, badge, headline, hint, labelOf, laneText, result, unbound, type Calibration } from "./addmonitor";
import type { CalibrationView, Desk, DisplayView } from "./model";

const DESK: Desk = {
  monitors: [
    { id: "m34", label: '34"', wiring: { pc: "displayport", mac: "hdmi" } },
    { id: "m32", label: '32"', wiring: { pc: "displayport", mac: "hdmi" } },
  ],
};

const STARTED: CalibrationView = { type: "started", display: 1, name: "Odyssey G5", monitor: "m32", start: "displayport" };

/** El recorrido del motor con el 32" (T-077): el cambio a mano, la vuelta y el HDMI. */
const RUN_32: CalibrationView[] = [
  STARTED,
  { type: "progress", step: { kind: "reading" } },
  { type: "progress", step: { kind: "user_switch", input: "hdmi" } },
  { type: "progress", step: { kind: "testing", input: "displayport" } },
  { type: "progress", step: { kind: "trying", input: "displayport", code: 0x0f, attempt: 1, of: 31 } },
  { type: "progress", step: { kind: "seen", input: "displayport", code: 0x0f, seen: true } },
  { type: "progress", step: { kind: "found", input: "displayport", code: 0x0f } },
  { type: "progress", step: { kind: "testing", input: "hdmi" } },
  { type: "progress", step: { kind: "trying", input: "hdmi", code: 0x05, attempt: 1, of: 30 } },
  { type: "progress", step: { kind: "seen", input: "hdmi", code: 0x05, seen: true } },
  { type: "progress", step: { kind: "returned", code: 0x0f } },
  { type: "progress", step: { kind: "found", input: "hdmi", code: 0x05 } },
  { type: "progress", step: { kind: "verifying" } },
  { type: "progress", step: { kind: "verified", input: "displayport", code: 0x0f } },
  { type: "progress", step: { kind: "verified", input: "hdmi", code: 0x05 } },
  { type: "progress", step: { kind: "verified", input: "displayport", code: 0x0f } },
];

function play(events: CalibrationView[], from: Calibration | null = null): Calibration | null {
  return events.reduce<Calibration | null>(advance, from);
}

describe("avance de la calibración", () => {
  it("cada entrada pasa por sus estados hasta su código", () => {
    const after = (n: number) => play(RUN_32.slice(0, n))!;
    expect(after(1).lanes.hdmi.state).toBe("waiting");
    expect(headline(after(2), '32"')).toBe('Leyendo el 32"…');

    const asked = after(3);
    expect(asked.lanes.hdmi.state).toBe("switch");
    expect(laneText(asked.lanes.hdmi)).toBe("cámbialo desde el menú del monitor");
    expect(headline(asked, '32"')).toBe('Pasa el 32" a HDMI desde su menú');
    expect(hint(asked)).toContain("haz clic en su aviso");

    const trying = after(5);
    // El HDMI ya se vio a mano: espera sus pruebas mientras se busca la vuelta.
    expect(trying.lanes.hdmi.state).toBe("waiting");
    expect(laneText(trying.lanes.displayport)).toBe("probando 0x0F · 1 de 31");
    expect(headline(trying, '32"')).toBe("Probando códigos de DisplayPort…");

    const back = after(11);
    expect(back.lanes.displayport).toMatchObject({ state: "found", code: 0x0f });
    expect(headline(back, '32"')).toBe("Volviendo a DisplayPort…");

    const done = after(12);
    expect(laneText(done.lanes.hdmi)).toBe("✓ 0x05");
    expect(laneText(done.lanes.displayport)).toBe("✓ 0x0F");
    expect(hint(done)).toContain("di qué ves en el monitor: la Mac, la PC o negro");

    // T-081: la ida y vuelta final antes de dar el perfil por bueno.
    const verifying = after(14);
    expect(headline(verifying, '32"')).toBe("Verificando: ida y vuelta con los dos códigos…");
    expect(verifying.lanes.hdmi).toMatchObject({ state: "found", code: 0x05 });
    expect(headline(after(RUN_32.length), '32"')).toBe("Verificando: ida y vuelta con los dos códigos…");
  });

  it("termina con el resultado «32\": HDMI ✓ DP ✓»", () => {
    const saved = play([
      ...RUN_32,
      {
        type: "finished",
        outcome: { kind: "saved", name: "Odyssey G5 (SAM7489)", hdmi: 5, display_port: 15, attempts: 2, monitor: "m32", file: "/d/SAM7489.toml", toml: "x", notes: [] },
      },
    ])!;
    expect(headline(saved, '32"')).toBe('32": HDMI ✓ DP ✓');
    expect(hint(saved)).toBe("2 códigos probados. El próximo cambio de modo ya los usa.");
    // Lo que llegue después del final no lo cambia.
    expect(advance(saved, { type: "progress", step: { kind: "reading" } })).toBe(saved);
  });

  it("si falla, la entrada que se probaba queda sin código y la encontrada sigue", () => {
    const failed = play([
      ...RUN_32.slice(0, 9),
      { type: "finished", outcome: { kind: "failed", reason: "ningún código funcionó para hdmi" } },
    ])!;
    expect(failed.lanes.hdmi.state).toBe("failed");
    expect(laneText(failed.lanes.hdmi)).toBe("ningún código funcionó");
    expect(failed.lanes.displayport.state).toBe("found");
    expect(headline(failed, '32"')).toBe('No se pudo calibrar el 32"');
    expect(hint(failed)).toBe("Ningún código funcionó para hdmi. El monitor volvió a DisplayPort.");
  });

  it("al cancelar a mano nada queda pendiente", () => {
    const cancelled = play([...RUN_32.slice(0, 3), { type: "progress", step: { kind: "user_switch", input: "displayport" } }, { type: "finished", outcome: { kind: "cancelled" } }])!;
    expect(cancelled.lanes.hdmi.state).toBe("waiting");
    expect(headline(cancelled, "x")).toBe("Calibración cancelada");
    expect(hint(cancelled)).toContain("No se guardó nada");
    // Pedir la vuelta a mano a la entrada de partida dice «regresa».
    const back = play([...RUN_32.slice(0, 3), { type: "progress", step: { kind: "user_switch", input: "displayport" } }])!;
    expect(headline(back, '32"')).toBe('Regresa el 32" a DisplayPort desde su menú');
  });

  it("sin «started» no hay calibración que avanzar", () => {
    expect(advance(null, { type: "progress", step: { kind: "reading" } })).toBeNull();
    expect(result("34\"", true, false)).toBe('34": HDMI ✓ DP ✗');
  });
});

describe("lista de monitores", () => {
  const known: DisplayView = {
    index: 1,
    name: "Odyssey G5",
    pnp_id: "SAM7489",
    monitor: "m32",
    profile: { kind: "known", name: "Odyssey G5", user: false, hdmi: 5, display_port: 15, toml: "x" },
  };

  it("cada perfil tiene su insignia y sus códigos", () => {
    expect(badge({ kind: "configured", hdmi: 6, display_port: 9 })).toEqual({ text: "Configurado", kind: "ok", codes: "HDMI 0x06 · DP 0x09" });
    expect(badge(known.profile)).toMatchObject({ text: "En la base", kind: "known" });
    expect(badge({ ...known.profile, user: true } as DisplayView["profile"]).text).toBe("Perfil tuyo");
    expect(badge({ kind: "none" })).toEqual({ text: "Sin perfil", kind: "none", codes: null });
  });

  it("se ofrece calibrar lo que no tiene perfil y usar lo de la base", () => {
    expect(actions({ ...known, profile: { kind: "none" } })).toEqual(["calibrate"]);
    expect(actions(known)).toEqual(["use", "calibrate", "export"]);
    // Sin monitor de la configuración no hay a qué aplicarlo.
    expect(actions({ ...known, monitor: null })).toEqual(["calibrate", "export"]);
    expect(actions({ ...known, profile: { kind: "configured", hdmi: 5, display_port: 15 } })).toEqual(["recalibrate"]);
  });

  it("nombra con la etiqueta de la configuración y ofrece los monitores libres", () => {
    expect(labelOf(DESK, "m32", "Odyssey G5")).toBe('32"');
    expect(labelOf(DESK, null, "U2720Q")).toBe("U2720Q");
    expect(labelOf(null, "m32", "Odyssey G5")).toBe("Odyssey G5");
    expect(unbound(DESK, [known])).toEqual([{ id: "m34", label: '34"' }]);
    expect(unbound(null, [])).toEqual([]);
  });
});
