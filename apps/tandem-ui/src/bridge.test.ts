import { expect, it } from "vitest";

import { bridgeFailed } from "./bridge";

it("convierte un fallo del puente en un estado sin agente", () => {
  expect(bridgeFailed(new Error("sin __TAURI_INTERNALS__"))).toEqual({
    kind: "unavailable",
    endpoint: null,
    reason: "la ventana no pudo preguntarle al núcleo de la app: sin __TAURI_INTERNALS__",
  });
});
