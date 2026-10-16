import { describe, expect, it } from "vitest";

import type { ProbeShow } from "./model";
import { countdown, demoProbe, hex, nextQuestion, probeText, questionText, readProbe, secondsLeft } from "./probe";

const CANDIDATE: ProbeShow = {
  type: "show",
  id: 3,
  monitor: "Odyssey G5",
  input: "hdmi",
  ask: { kind: "candidate", code: 0x05 },
  timeout_ms: 8000,
  place: { kind: "monitor", screen: null },
};
const SWITCH: ProbeShow = { ...CANDIDATE, id: 4, input: "displayport", ask: { kind: "user_switch" }, timeout_ms: 120000 };
const QUESTION: ProbeShow = { ...CANDIDATE, id: 5, place: { kind: "question" } };

describe("sonda a pantalla completa", () => {
  it("pregunta si se ve, con el monitor, la entrada y el código", () => {
    expect(probeText(CANDIDATE)).toEqual({
      title: "¿Ves esta pantalla?",
      detail: "Calibrando el Odyssey G5: entrada HDMI, código 0x05.",
      button: "Sí, la veo",
    });
  });

  it("en el cambio a mano dice qué hacer en el menú del monitor", () => {
    const t = probeText(SWITCH);
    expect(t.title).toBe("Cuando veas esta pantalla, haz clic");
    expect(t.detail).toBe("Cambia el Odyssey G5 a DisplayPort desde el menú del monitor.");
    expect(t.button).toBe("La veo");
  });

  it("los códigos van en hexadecimal de dos cifras", () => {
    expect(hex(0x0f)).toBe("0x0F");
    expect(hex(0x11)).toBe("0x11");
    expect(hex(0)).toBe("0x00");
  });
});

describe("plazo", () => {
  it("cuenta los segundos que faltan, redondeando hacia arriba, sin bajar de cero", () => {
    expect(secondsLeft(8000, 0)).toBe(8);
    expect(secondsLeft(8000, 200)).toBe(8);
    expect(secondsLeft(8000, 7001)).toBe(1);
    expect(secondsLeft(8000, 8000)).toBe(0);
    expect(secondsLeft(8000, 9000)).toBe(0);
  });

  it("sin clic a tiempo dice que cuenta como no vista", () => {
    expect(countdown(3)).toContain("3 s");
    expect(countdown(0)).toBe("Sin respuesta: cuenta como no vista.");
  });
});

describe("pregunta de respaldo", () => {
  it("pregunta si el monitor cambió, con Sí y No", () => {
    expect(questionText(QUESTION)).toEqual({ text: "¿El Odyssey G5 pasó a HDMI? (código 0x05)", yes: "Sí, cambió", no: "No" });
    expect(questionText({ ...SWITCH, place: { kind: "question" } })).toEqual({
      text: "Cambia el Odyssey G5 a DisplayPort desde el menú del monitor. ¿Ya cambió?",
      yes: "Ya cambió",
      no: "No puedo",
    });
  });

  it("solo las preguntas llegan a la ventana principal, y se van con su cierre", () => {
    expect(nextQuestion(null, CANDIDATE)).toBeNull();
    expect(nextQuestion(null, QUESTION)).toBe(QUESTION);
    // La sonda de otro monitor no la tapa; el cierre de otra no la quita.
    expect(nextQuestion(QUESTION, CANDIDATE)).toBe(QUESTION);
    // Una sonda fuera del escritorio no es una pregunta.
    expect(nextQuestion(null, { ...CANDIDATE, place: { kind: "offscreen" } })).toBeNull();
    expect(nextQuestion(QUESTION, { type: "hide", id: CANDIDATE.id })).toBe(QUESTION);
    expect(nextQuestion(QUESTION, { type: "hide", id: QUESTION.id })).toBeNull();
  });
});

describe("lo que recibe la ventana", () => {
  it("lee la sonda que dejó la app y rechaza lo que no lo es", () => {
    expect(readProbe(JSON.parse(JSON.stringify(CANDIDATE)))).toEqual(CANDIDATE);
    expect(readProbe(undefined)).toBeNull();
    expect(readProbe({ type: "hide", id: 3 })).toBeNull();
    expect(readProbe({ ...CANDIDATE, id: "3" })).toBeNull();
    expect(readProbe({ ...CANDIDATE, timeout_ms: undefined })).toBeNull();
  });

  it("el modo demo arma una sonda de muestra", () => {
    expect(demoProbe("?demo")).toEqual({ ...CANDIDATE, id: 1 });
    const s = demoProbe("?demo&ask=switch&input=dp&monitor=LC34G55T&place=question");
    expect(s).toMatchObject({ monitor: "LC34G55T", input: "displayport", ask: { kind: "user_switch" }, place: { kind: "question" } });
  });
});
