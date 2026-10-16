import { describe, expect, it } from "vitest";

import type { ProbeAnswer, ProbeShow } from "./model";
import { countdown, demoProbe, hex, nextQuestion, probeText, questionText, readProbe, secondsLeft } from "./probe";

const CANDIDATE: ProbeShow = {
  type: "show",
  id: 3,
  monitor: "Odyssey G5",
  input: "hdmi",
  machine: "mac",
  ask: { kind: "candidate", code: 0x05 },
  timeout_ms: 20000,
  place: { kind: "monitor", screen: null },
};
const SWITCH: ProbeShow = { ...CANDIDATE, id: 4, input: "displayport", machine: "pc", ask: { kind: "user_switch" }, timeout_ms: 120000 };
const QUESTION: ProbeShow = { ...CANDIDATE, id: 5, place: { kind: "question" } };

describe("sonda a pantalla completa", () => {
  it("pregunta si se ve, con el monitor, la entrada, la máquina y el código", () => {
    expect(probeText(CANDIDATE)).toEqual({
      title: "¿Ves esta pantalla?",
      detail: "Calibrando el Odyssey G5: mandé 0x05 para HDMI (la Mac).",
      button: "Sí, la veo",
    });
  });

  it("en el cambio a mano dice qué hacer en el menú del monitor", () => {
    const t = probeText(SWITCH);
    expect(t.title).toBe("Cuando veas esta pantalla, haz clic");
    expect(t.detail).toBe("Cambia el Odyssey G5 a DisplayPort desde el menú del monitor: aquí se ve la PC.");
    expect(t.button).toBe("La veo");
  });

  it("la vuelta y la verificación dicen qué máquina debería verse", () => {
    expect(probeText({ ...SWITCH, ask: { kind: "back", code: 0x0f } }).detail).toBe(
      "Volví con 0x0F: el Odyssey G5 debería mostrar la PC.",
    );
    expect(probeText({ ...CANDIDATE, ask: { kind: "verify", code: 0x05 } }).detail).toContain("Verificación del Odyssey G5");
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

  it("sin respuesta no dice que no se vio: se repite el código", () => {
    expect(countdown(3)).toBe("Quedan 3 s para contestar.");
    expect(countdown(0)).toBe("Sin respuesta: se repite el código y después se pregunta si seguir.");
  });
});

describe("pregunta «¿qué ves?»", () => {
  it("nombra la máquina que debería verse y ofrece una respuesta por máquina, negro y parar", () => {
    const q = questionText(QUESTION);
    expect(q.text).toBe("Mandé 0x05 para HDMI (la Mac). ¿Qué ves en el Odyssey G5?");
    expect(q.choices.map((c) => c.label)).toEqual(["Veo la Mac", "Veo la PC", "Negro o sin señal", "Parar"]);
    expect(q.choices.map((c) => c.answer)).toEqual<ProbeAnswer[]>([
      { kind: "machine", machine: "mac" },
      { kind: "machine", machine: "pc" },
      { kind: "black" },
      { kind: "stop" },
    ]);
    // Ya no hay «Sí, cambió»: no dice qué se ve.
    expect(q.choices.some((c) => c.label.startsWith("Sí"))).toBe(false);
  });

  it("cada tipo de pregunta explica qué pasó", () => {
    const text = (ask: ProbeShow["ask"], base: ProbeShow = QUESTION) => questionText({ ...base, ask }).text;
    expect(text({ kind: "user_switch" }, { ...SWITCH, place: { kind: "question" } })).toBe(
      "Pasa el Odyssey G5 a DisplayPort desde su menú para ver la PC. Cuando cambie, ¿qué ves?",
    );
    expect(text({ kind: "back", code: 0x0f })).toBe("Volví con 0x0F: debería verse la Mac. ¿Qué ves en el Odyssey G5?");
    expect(text({ kind: "verify", code: 0x05 })).toBe("Verificación: mandé 0x05, debería verse la Mac. ¿Qué ves en el Odyssey G5?");
    expect(text({ kind: "stalled", code: 0x05 })).toBe(
      "Nadie contestó tras mandar 0x05 dos veces. ¿Sigo? Di qué ves en el Odyssey G5, o para.",
    );
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
    expect(readProbe({ ...CANDIDATE, machine: undefined })).toBeNull();
  });

  it("el modo demo arma una sonda de muestra", () => {
    expect(demoProbe("?demo")).toEqual({ ...CANDIDATE, id: 1 });
    const s = demoProbe("?demo&ask=switch&input=dp&monitor=LC34G55T&place=question");
    expect(s).toMatchObject({ monitor: "LC34G55T", input: "displayport", ask: { kind: "user_switch" }, place: { kind: "question" } });
  });
});
