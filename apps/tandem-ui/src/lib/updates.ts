// Ajustes → Actualizaciones (`F-Ajustes`, T-122): lo que se muestra a partir de la vista
// que manda el núcleo de la app (`tandem_ui::update::UpdateView`).

/** Un paso de «Actualizar todas» (`tandem_ui::update::Step`). */
export type UpdateStep = "ask_peer" | "wait_peer" | "install_here";

/** Espejo de `tandem_ui::update::UpdateView`. */
export interface UpdateView {
  /** La versión de esta máquina. */
  here: string;
  /** La de la otra (`null` sin par o si todavía no se sabe). */
  peer: string | null;
  paired: boolean;
  /** La versión nueva encontrada, si alguna de las dos está atrás. */
  available: string | null;
  checking: boolean;
  step: UpdateStep | null;
  error: string | null;
  /** «Actualizar solo». */
  auto: boolean;
}

export const STEP_TEXT: Record<UpdateStep, string> = {
  ask_peer: "Pidiendo a la otra máquina que se actualice…",
  wait_peer: "La otra máquina se está actualizando; esta sigue con el teclado y el mouse…",
  install_here: "Instalando en esta máquina; la app se reinicia sola…",
};

/** «Tandem 0.3.0» y la línea de abajo: si las dos máquinas están en la misma versión. */
export function versionRow(v: UpdateView): { title: string; detail: string } {
  const title = `Tandem ${v.here}`;
  if (!v.paired) return { title, detail: "Esta máquina trabaja sola." };
  if (v.peer === null) return { title, detail: "Todavía no se sabe la versión de la otra máquina." };
  if (v.peer === v.here) return { title, detail: "Todas las máquinas del puesto en la misma versión." };
  return { title, detail: `La otra máquina tiene la ${v.peer}.` };
}

export function checkLabel(v: UpdateView, checkedOnce: boolean): string {
  if (v.checking) return "Buscando…";
  return checkedOnce ? "Buscar de nuevo" : "Buscar ahora";
}

/** La fila «Hay una versión nueva», o nada. */
export function availableRow(v: UpdateView): { title: string; detail: string } | null {
  if (!v.available) return null;
  const detail = v.step ? STEP_TEXT[v.step] : v.paired ? "Se instala en todas las máquinas, una a la vez, sin cortar el teclado." : "Se instala en esta máquina.";
  return { title: `Hay una versión nueva: ${v.available}`, detail };
}

/** Lo que se ve tras buscar sin encontrar nada nuevo. */
export function upToDate(v: UpdateView, checkedOnce: boolean): boolean {
  return checkedOnce && !v.checking && !v.available && !v.error;
}
