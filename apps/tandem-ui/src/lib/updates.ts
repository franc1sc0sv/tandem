// Ajustes → Actualizaciones (`F-Settings`, T-122): lo que se muestra a partir de la vista
// que manda el núcleo de la app (`tandem_ui::update::UpdateView`).

/** Un paso de «Update all» (`tandem_ui::update::Step`). */
export type UpdateStep = "ask_peer" | "wait_peer" | "install_here";

/** Espejo de `tandem_ui::update::UpdateView`. */
export interface UpdateView {
  /** La versión de this machine. */
  here: string;
  /** La de la otra (`null` sin par o si todavía no se sabe). */
  peer: string | null;
  paired: boolean;
  /** La versión nueva encontrada, si alguna de las dos está atrás. */
  available: string | null;
  checking: boolean;
  step: UpdateStep | null;
  error: string | null;
  /** «Update only». */
  auto: boolean;
}

export const STEP_TEXT: Record<UpdateStep, string> = {
  ask_peer: "Asking the other machine to update…",
  wait_peer: "The other machine is updating; this one keeps the keyboard and mouse…",
  install_here: "Installing on this machine; the app restarts on its own…",
};

/** «Tandem 0.3.0» y la línea de abajo: si las dos máquinas están en la misma versión. */
export function versionRow(v: UpdateView): { title: string; detail: string } {
  const title = `Tandem ${v.here}`;
  if (!v.paired) return { title, detail: "This machine works alone." };
  if (v.peer === null) return { title, detail: "The other machine’s version is not known yet." };
  if (v.peer === v.here) return { title, detail: "All machines of the desk on the same version." };
  return { title, detail: `The other machine has ${v.peer}.` };
}

export function checkLabel(v: UpdateView, checkedOnce: boolean): string {
  if (v.checking) return "Checking…";
  return checkedOnce ? "Check again" : "Check now";
}

/** La fila «Hay una versión nueva», o nada. */
export function availableRow(v: UpdateView): { title: string; detail: string } | null {
  if (!v.available) return null;
  const detail = v.step ? STEP_TEXT[v.step] : v.paired ? "It installs on every machine, one at a time, without cutting the keyboard." : "It installs on this machine.";
  return { title: `A new version is available: ${v.available}`, detail };
}

/** Lo que se ve tras buscar sin encontrar nada nuevo. */
export function upToDate(v: UpdateView, checkedOnce: boolean): boolean {
  return checkedOnce && !v.checking && !v.available && !v.error;
}
