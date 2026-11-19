// What the agent announces about a machine that the demo has to repeat exactly: which modules
// have a switch and what each system says about the administrator service. The source is the
// agent (`crates/tandemd/src/modules.rs`, `fn switchable`, and `admin_service.rs`, `check`);
// `capabilities.test.ts` reads those files and fails if this copy drifts.

import type { Os } from "./model";

/** The modules the user can turn on and off (T-098); the others have a plain chip, with no switch. */
export const SWITCHABLE_MODULES: readonly string[] = ["input", "clipboard"];

export const isSwitchable = (id: string): boolean => SWITCHABLE_MODULES.includes(id);

/** The note of the administrator service when the install is per-user, per system (`admin_service::check`). */
export function perUserAdminNote(os: Os): string {
  return os === "macos"
    ? "user-level installation does not support the administrator service: Tandem is not administrators-only. Move Tandem to Applications (owned by root) to install it"
    : "user-level installation: Tandem is not administrators-only. Install Tandem in Program Files for the service";
}

/** Whether the system asks for permissions to capture and inject input (only macOS does). */
export const hasInputPermissions = (os: Os): boolean => os === "macos";
