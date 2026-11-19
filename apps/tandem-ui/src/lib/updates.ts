// Settings → Updates (`F-Settings`, T-122): what is shown from the view
// the app core sends (`tandem_ui::update::UpdateView`).

/** A step of "Update all" (`tandem_ui::update::Step`). */
export type UpdateStep = "ask_peer" | "wait_peer" | "install_here";

/** Mirror of `tandem_ui::update::UpdateView`. */
export interface UpdateView {
  /**
   * Grows with every change of the view. The command replies and the events travel on different
   * channels: the window keeps the view with the highest one, not the last to arrive (T-251).
   */
  seq: number;
  /** This machine's version. */
  here: string;
  /** The other one's (`null` without a peer or if not known yet). */
  peer: string | null;
  paired: boolean;
  /** The new version found, if either one is behind. */
  available: string | null;
  checking: boolean;
  step: UpdateStep | null;
  error: string | null;
  /** «Update automatically». */
  auto: boolean;
}

export const STEP_TEXT: Record<UpdateStep, string> = {
  ask_peer: "Asking the other machine to update…",
  wait_peer: "The other machine is updating; this one keeps the keyboard and mouse…",
  install_here: "Installing on this machine; the app restarts on its own…",
};

/** "Tandem 0.3.0" and the line below: whether both machines are on the same version. */
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

/** The "A new version is available" row, or nothing. */
export function availableRow(v: UpdateView): { title: string; detail: string } | null {
  if (!v.available) return null;
  const detail = v.step ? STEP_TEXT[v.step] : v.paired ? "It installs on every machine, one at a time, without cutting the keyboard." : "It installs on this machine.";
  return { title: `A new version is available: ${v.available}`, detail };
}

/** What is seen after searching without finding anything new. */
export function upToDate(v: UpdateView, checkedOnce: boolean): boolean {
  return checkedOnce && !v.checking && !v.available && !v.error;
}
