// macOS permissions as the user sees them: the name shown in the Settings panel, and which
// binary they are granted to (T-090). No DOM.

import type { ActionResult, MachineCaps, Permission } from "./model";

export const PERMISSION_NAMES: Record<Permission, string> = {
  accessibility: "Accessibility",
  input_monitoring: "Input Monitoring",
  local_network: "Local Network",
};

/** The permission name as the Settings panel shows it. */
export const permissionLabel = (p: Permission) => `${PERMISSION_NAMES[p]}`;

/** Where it is granted. */
export const SETTINGS_PATH = "System Settings → Privacy & Security";

/** The binary the agent runs, or `null` if the agent did not say. */
export function agentBinary(here: MachineCaps | null): string | null {
  const exe = here?.input.process.exe;
  return exe ? exe : null;
}

/** "Grant the permission to this binary: <path>", with the pid to tell a restart apart. */
export function grantTo(here: MachineCaps | null): string | null {
  const exe = agentBinary(here);
  if (!exe || !here) return null;
  return `Grant the permission to this binary: ${exe} (pid ${here.input.process.pid})`;
}

/** What the agent knows about a permission: `unknown` if there is no agent or it does not check (local network). */
export type PermissionState = "granted" | "missing" | "unknown";

export function permissionState(here: MachineCaps | null, p: Permission): PermissionState {
  if (!here || p === "local_network") return "unknown";
  return here.input.missing.includes(p) ? "missing" : "granted";
}

/**
 * What to do when something is missing. macOS evaluates part of the permissions once per process, and the
 * list entry belongs to a specific binary: if it was already on but is still missing,
 * it is almost always an old entry (another version or path) and it has to be removed and
 * requested again.
 */
export const MISSING_HELP = [
  "1. Press “Request permissions”: macOS shows its dialog for this binary.",
  `2. Turn it on in ${SETTINGS_PATH}.`,
  "3. Press “Restart agent” so the permission you just gave takes effect.",
  "If it was already on and is still missing, remove the old entry with “−”, press “Request permissions” and restart the agent.",
].join(" ");

/** What the permission buttons do, without a DOM: each returns the error to show and the notice. */
export interface PermissionDeps {
  request(): Promise<ActionResult>;
  /** `null` outside the app (there is nothing to restart the agent with). */
  restartAgent: (() => Promise<void>) | null;
  openSettings: (() => Promise<void>) | null;
}

export interface PermissionOutcome {
  error: string | null;
  message?: string;
}

export async function pressPermission(action: string, d: PermissionDeps): Promise<PermissionOutcome> {
  if (action === "request") {
    const r = await d.request();
    return { error: r.kind === "rejected" ? r.reason : r.kind === "unavailable" ? `no agent: ${r.reason}` : null };
  }
  if (action === "restart") {
    if (!d.restartAgent) return { error: "Could not restart the agent: the app is not available" };
    try {
      await d.restartAgent();
      return { error: null, message: "tandemd is restarting: permissions are re-checked in a few seconds" };
    } catch (e) {
      return { error: `Could not restart the agent: ${e instanceof Error ? e.message : String(e)}` };
    }
  }
  await d.openSettings?.();
  return { error: null };
}
