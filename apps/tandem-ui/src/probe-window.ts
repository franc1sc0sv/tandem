// La ventana de la sonda de calibración (`probe.html`): la abre la app sobre el monitor que se
// calibra con la sonda ya puesta (`window.__TANDEM_PROBE__`) y la cierra cuando el agente deja
// de esperarla. Un clic en el botón (o Enter) es «la veo».
//
// Modo demo, para capturas en el navegador: `pnpm dev` y `/probe.html?demo` (`&ask=switch`,
// `&input=dp`, `&monitor=LC34G55T`, `&timeout=8000`).

import "./styles.css";
import "./probe.css";

import type { Action, ProbeShow } from "./model";
import { countdown, demoProbe, probeText, readProbe, secondsLeft } from "./probe";

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const TICK_MS = 250;

async function answerer(): Promise<(a: Action) => Promise<unknown>> {
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("demo")) {
    return async (a) => console.info("demo: la sonda contestaría", a);
  }
  const { tauriBridge } = await import("./bridge");
  const bridge = await tauriBridge();
  return (a) => bridge.act(a);
}

function show(p: ProbeShow, answer: (a: Action) => Promise<unknown>) {
  const text = probeText(p);
  $("probe-title").textContent = text.title;
  $("probe-detail").textContent = text.detail;
  $("probe-button").textContent = text.button;
  const button = $("probe-seen") as HTMLButtonElement;
  const started = performance.now();
  let answered = false;

  const tick = () => {
    const elapsed = performance.now() - started;
    const left = secondsLeft(p.timeout_ms, elapsed);
    if (!answered) $("probe-count").textContent = countdown(left);
    const rest = Math.max(0, 1 - elapsed / p.timeout_ms);
    $("probe-bar").style.transform = `scaleX(${rest})`;
    if (left === 0) {
      button.disabled = true;
      window.clearInterval(timer);
    }
  };
  const timer = window.setInterval(tick, TICK_MS);
  tick();

  button.addEventListener("click", () => {
    if (answered) return;
    answered = true;
    button.classList.add("done");
    button.disabled = true;
    $("probe-button").textContent = "Listo";
    $("probe-count").textContent = "Gracias: sigue la calibración.";
    void answer({ kind: "answer_probe", id: p.id, seen: true });
  });
  button.focus();
}

async function boot() {
  const injected = (window as unknown as Record<string, unknown>).__TANDEM_PROBE__;
  const demo = import.meta.env.DEV && new URLSearchParams(location.search).has("demo");
  const probe = readProbe(injected) ?? (demo ? demoProbe(location.search) : null);
  if (!probe) {
    $("probe-title").textContent = "No hay ninguna sonda que mostrar";
    $("probe-seen").hidden = true;
    return;
  }
  show(probe, await answerer());
}

void boot();
