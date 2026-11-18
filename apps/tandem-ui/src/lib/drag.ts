// El aviso del arrastre de archivos entre las máquinas (T-121). Sin mockup propio: se dibuja
// con el aviso corto de siempre (`Toasts.svelte`), con el texto del avance. Puro.

import type { DragStatus } from "./model";

export interface DragNotice {
  text: string;
  tone: "ok" | "bad";
  /** El arrastre sigue: el aviso se queda hasta el siguiente estado, no se cierra solo. */
  ongoing: boolean;
}

function files(n: number): string {
  return n === 1 ? "1 archivo" : `${n} archivos`;
}

export function dragNotice(s: DragStatus): DragNotice {
  switch (s.kind) {
    case "copying":
      return { text: `Copiando ${s.index + 1} de ${s.total}: ${s.name}`, tone: "ok", ongoing: true };
    case "copied":
      return { text: `Listo para soltar: ${files(s.total)}`, tone: "ok", ongoing: true };
    case "dropped":
      return { text: "Archivos copiados a la otra máquina", tone: "ok", ongoing: false };
    case "cancelled":
      return { text: "Arrastre cancelado", tone: "ok", ongoing: false };
    case "failed":
      return { text: `No se pudo arrastrar: ${s.reason}`, tone: "bad", ongoing: false };
  }
}
