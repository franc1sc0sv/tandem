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
  /** Installing here asks for administrator once (Windows, Tandem under Program Files; T-348). */
  admin: boolean;
  /** This app was just updated from this version: the result of the last "Update" (T-348). */
  updated_from: string | null;
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

/** What installing asks for, beyond the click (T-348). */
export const ADMIN_TEXT = "This PC asks for administrator once, to update Tandem in Program Files and its service.";

/** How the update installs, before it starts. */
function howItInstalls(v: UpdateView): string {
  const how = v.paired ? "It installs on every machine, one at a time, without cutting the keyboard." : "It installs on this machine.";
  return v.admin ? `${how} ${ADMIN_TEXT}` : how;
}

/** The "A new version is available" row, or nothing. */
export function availableRow(v: UpdateView): { title: string; detail: string } | null {
  if (!v.available) return null;
  const detail = v.step ? STEP_TEXT[v.step] : howItInstalls(v);
  return { title: `A new version is available: ${v.available}`, detail };
}

/** The app-wide notice (T-348): "Update available: vX.Y.Z", one button and the result. */
export interface UpdateNotice {
  title: string;
  detail: string;
  /** The button's text: it runs "Update all" (the other machine first, then this one). */
  action: string;
  /** An update is running: the button waits. */
  busy: boolean;
  /** The last attempt failed (rollback: this machine stayed on its version). */
  failed: boolean;
}

/**
 * The notice while there is a new version, or nothing. `starting`: the button was pressed and
 * its reply has not come yet. `error`: what the command itself rejected with.
 */
export function updateNotice(v: UpdateView, starting = false, error: string | null = null): UpdateNotice | null {
  if (!v.available) return null;
  const title = `Update available: v${v.available}`;
  const action = v.paired ? "Update all" : "Update";
  const busy = starting || v.step !== null;
  const failure = error ?? v.error;
  if (v.step) return { title, detail: STEP_TEXT[v.step], action: "Updating…", busy: true, failed: false };
  if (busy) return { title, detail: STEP_TEXT[v.paired ? "ask_peer" : "install_here"], action: "Updating…", busy, failed: false };
  if (failure) return { title, detail: `The update did not finish: ${failure}`, action: "Try again", busy, failed: true };
  return { title, detail: howItInstalls(v), action, busy, failed: false };
}

/** The result after the restart (T-348): "Updated to vX.Y.Z" and where each machine is, or nothing. */
export function updatedResult(v: UpdateView): { text: string; body: string } | null {
  if (!v.updated_from) return null;
  const text = `Updated to v${v.here}`;
  if (!v.paired) return { text, body: `From v${v.updated_from}.` };
  if (v.peer === v.here) return { text, body: `From v${v.updated_from}. Both machines are on v${v.here}.` };
  if (v.peer === null) return { text, body: `From v${v.updated_from}.` };
  return { text, body: `From v${v.updated_from}. The other machine has v${v.peer}.` };
}

/** What is seen after searching without finding anything new. */
export function upToDate(v: UpdateView, checkedOnce: boolean): boolean {
  return checkedOnce && !v.checking && !v.available && !v.error;
}
