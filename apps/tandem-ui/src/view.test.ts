import { describe as suite, expect, it } from "vitest";

import { describe, invokeFailed, type AgentStatus } from "./view";

suite("describe", () => {
  it("muestra el estado del agente conectado", () => {
    const status: AgentStatus = {
      kind: "connected",
      agent: "mac",
      state: { owner: { m34: "pc", m32: "mac" }, pc_on: true, builtin_pref: "off" },
    };
    const view = describe(status);
    expect(view.kind).toBe("connected");
    if (view.kind !== "connected") return;
    expect(view.pill).toBe("tandemd · Mac");
    expect(view.rows).toEqual([
      { label: "PC", value: "Encendida", accent: "pc" },
      { label: "Monitor m32", value: "Mac", accent: "mac" },
      { label: "Monitor m34", value: "PC", accent: "pc" },
      { label: "Pantalla integrada", value: "Apagada", accent: "mac" },
    ]);
  });

  it("avisa si no hay monitores", () => {
    const view = describe({
      kind: "connected",
      agent: "pc",
      state: { owner: {}, pc_on: false, builtin_pref: "extend" },
    });
    expect(view.title).toBe("Sin monitores configurados");
  });

  it("explica por qué no hay agente", () => {
    const view = describe({
      kind: "unavailable",
      endpoint: "/tmp/tandemd.sock",
      reason: "tandemd no está corriendo",
    });
    expect(view).toEqual({
      kind: "unavailable",
      pill: "Sin agente",
      title: "No se pudo leer el estado de tandemd",
      reason: "tandemd no está corriendo",
      endpoint: "/tmp/tandemd.sock",
    });
  });

  it("convierte un fallo de invoke en un estado sin agente", () => {
    const status = invokeFailed(new Error("sin __TAURI_INTERNALS__"));
    expect(status).toEqual({
      kind: "unavailable",
      endpoint: null,
      reason: "la ventana no pudo preguntarle al núcleo de la app: sin __TAURI_INTERNALS__",
    });
  });
});
