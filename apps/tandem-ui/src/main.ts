import { invoke } from "@tauri-apps/api/core";

import "./styles.css";
import { describe, invokeFailed, type AgentStatus, type View } from "./view";

// Hasta que el agente empuje cambios a la ventana (T-041), se vuelve a preguntar cada tanto.
const REFRESH_MS = 3000;

const stage = document.getElementById("stage")!;
const pillText = document.getElementById("agent-text")!;
const pillDot = document.getElementById("agent-dot")!;

function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string) {
  const node = document.createElement(tag);
  if (cls) node.className = cls;
  if (text !== undefined) node.textContent = text;
  return node;
}

function render(view: View) {
  pillText.textContent = view.pill;
  pillDot.classList.toggle("warn", view.kind !== "connected");

  const card = el("section", `card ${view.kind}`);
  card.append(el("h1", "title", view.title));
  if (view.kind === "connected") {
    const list = el("dl", "rows");
    for (const row of view.rows) {
      const item = el("div", `row${row.accent ? ` ${row.accent}` : ""}`);
      item.append(el("dt", undefined, row.label), el("dd", undefined, row.value));
      list.append(item);
    }
    card.append(list);
  } else {
    card.append(el("p", "reason", view.reason));
    if (view.endpoint) {
      const where = el("p", "endpoint", "Buscado en ");
      where.append(el("code", undefined, view.endpoint));
      card.append(where);
    }
    const retry = el("button", "pill btn", "Reintentar");
    retry.addEventListener("click", () => void refresh());
    card.append(retry);
  }
  stage.replaceChildren(card);
}

let busy = false;

async function refresh() {
  if (busy) return;
  busy = true;
  try {
    const status = await invoke<AgentStatus>("agent_status").catch(invokeFailed);
    render(describe(status));
  } finally {
    busy = false;
  }
}

void refresh();
setInterval(() => void refresh(), REFRESH_MS);
