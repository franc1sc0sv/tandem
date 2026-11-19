// The notice of the file drag between the machines (T-121). No mockup of its own: it is drawn
// with the usual short notice (`Toasts.svelte`), with the progress text. Pure.

import type { DragStatus } from "./model";

export interface DragNotice {
  text: string;
  tone: "ok" | "bad";
  /** The drag goes on: the notice stays until the next state, it does not close by itself. */
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
