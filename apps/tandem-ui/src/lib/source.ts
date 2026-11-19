// Qué fuente usa la ventana: el modo demo (`?demo`, solo si está en el build) o el agente
// real por Tauri.

import { bridgeFailed, tauriBridge, type Bridge } from "./bridge";
import { DemoSource, demoOptions, isDemo } from "./store/demo";
import { LiveSource } from "./store/live";
import type { Source } from "./store/types";

/** Un puente que solo informa que no hay agente (abierta en un navegador sin Tauri). */
function deadBridge(reason: string): Bridge {
  const snap = bridgeFailed(new Error(reason));
  const never = new Promise<never>(() => {});
  return new Proxy({} as Bridge, {
    get: (_t, prop) => {
      if (prop === "snapshot") return async () => snap;
      if (prop === "onSnapshot") return async () => {};
      return () => never;
    },
  });
}

export async function chooseSource(search: string, hasTauri: boolean): Promise<Source> {
  if (__DEMO__ && (isDemo(search) || !hasTauri)) return new DemoSource(demoOptions(search));
  if (!hasTauri) return new LiveSource(deadBridge("the window is not running inside Tauri"));
  try {
    return new LiveSource(await tauriBridge());
  } catch (e) {
    return new LiveSource(deadBridge(e instanceof Error ? e.message : String(e)));
  }
}

export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}
