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
  return n === 1 ? "1 file" : `${n} files`;
}

export function dragNotice(s: DragStatus): DragNotice {
  switch (s.kind) {
    case "copying":
      return { text: `Copying ${s.index + 1} of ${s.total}: ${s.name}`, tone: "ok", ongoing: true };
    case "copied":
      return { text: `Ready to drop: ${files(s.total)}`, tone: "ok", ongoing: true };
    case "dropped":
      return { text: "Files copied to the other machine", tone: "ok", ongoing: false };
    case "cancelled":
      return { text: "Drag cancelled", tone: "ok", ongoing: false };
    case "failed":
      return { text: `Could not drag: ${s.reason}`, tone: "bad", ongoing: false };
  }
}
