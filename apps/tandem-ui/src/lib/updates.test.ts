import { describe, expect, it } from "vitest";
import { ADMIN_TEXT, STEP_TEXT, availableRow, checkLabel, upToDate, updateNotice, updatedResult, versionRow, type UpdateView } from "./updates";

const base: UpdateView = { seq: 1, here: "0.3.0", peer: "0.3.0", paired: true, available: null, checking: false, step: null, error: null, auto: true, admin: false, updated_from: null };

describe("Updates", () => {
  it("says whether both machines are on the same version", () => {
    expect(versionRow(base)).toEqual({ title: "Tandem 0.3.0", detail: "All machines of the desk on the same version." });
    expect(versionRow({ ...base, peer: "0.3.1" }).detail).toBe("The other machine has 0.3.1.");
    expect(versionRow({ ...base, peer: null }).detail).toMatch(/not known yet/);
    expect(versionRow({ ...base, paired: false, peer: null }).detail).toBe("This machine works alone.");
  });

  it("the button changes from \"Check now\" to \"Check again\"", () => {
    expect(checkLabel(base, false)).toBe("Check now");
    expect(checkLabel(base, true)).toBe("Check again");
    expect(checkLabel({ ...base, checking: true }, true)).toBe("Checking…");
  });

  it("the new version says how it is installed and which step it is on", () => {
    expect(availableRow(base)).toBeNull();
    const found = { ...base, available: "0.3.1" };
    expect(availableRow(found)).toEqual({
      title: "A new version is available: 0.3.1",
      detail: "It installs on every machine, one at a time, without cutting the keyboard.",
    });
    expect(availableRow({ ...found, paired: false })?.detail).toBe("It installs on this machine.");
    expect(availableRow({ ...found, step: "wait_peer" })?.detail).toMatch(/this one keeps the keyboard/);
  });

  it("up to date only after searching and without errors", () => {
    expect(upToDate(base, false)).toBe(false);
    expect(upToDate(base, true)).toBe(true);
    expect(upToDate({ ...base, error: "no network" }, true)).toBe(false);
    expect(upToDate({ ...base, available: "0.3.1" }, true)).toBe(false);
  });
});

describe("Update notice and result (T-348)", () => {
  const found: UpdateView = { ...base, available: "0.3.1" };

  it("nothing to offer, no notice", () => {
    expect(updateNotice(base)).toBeNull();
  });

  it("offers \"Update all\" with a peer and \"Update\" alone", () => {
    expect(updateNotice(found)).toEqual({
      title: "Update available: v0.3.1",
      detail: "It installs on every machine, one at a time, without cutting the keyboard.",
      action: "Update all",
      busy: false,
      failed: false,
    });
    expect(updateNotice({ ...found, paired: false, peer: null })).toMatchObject({ action: "Update", detail: "It installs on this machine." });
  });

  it("says when this PC asks for administrator once", () => {
    expect(updateNotice({ ...found, admin: true })?.detail).toBe(`It installs on every machine, one at a time, without cutting the keyboard. ${ADMIN_TEXT}`);
    expect(availableRow({ ...found, admin: true })?.detail).toContain(ADMIN_TEXT);
  });

  it("while it runs: the step and a waiting button", () => {
    expect(updateNotice({ ...found, step: "wait_peer" })).toMatchObject({ detail: STEP_TEXT.wait_peer, action: "Updating…", busy: true, failed: false });
    // Pressed, reply not here yet: already waiting, with the first step of the plan.
    expect(updateNotice(found, true)).toMatchObject({ detail: STEP_TEXT.ask_peer, busy: true });
    expect(updateNotice({ ...found, paired: false }, true)).toMatchObject({ detail: STEP_TEXT.install_here, busy: true });
    // A step wins over a stale error.
    expect(updateNotice({ ...found, step: "ask_peer", error: "old" })?.failed).toBe(false);
  });

  it("a failed run says why and offers to try again", () => {
    expect(updateNotice({ ...found, error: "the other machine did not come back" })).toMatchObject({
      detail: "The update did not finish: the other machine did not come back",
      action: "Try again",
      busy: false,
      failed: true,
    });
    expect(updateNotice(found, false, "an update is already in progress")?.detail).toBe("The update did not finish: an update is already in progress");
  });

  it("after the restart: updated to the new version, and where the other machine is", () => {
    expect(updatedResult(base)).toBeNull();
    const done = { ...base, here: "0.3.1", peer: "0.3.1", updated_from: "0.3.0" };
    expect(updatedResult(done)).toEqual({ text: "Updated to v0.3.1", body: "From v0.3.0. Both machines are on v0.3.1." });
    expect(updatedResult({ ...done, peer: "0.3.0" })?.body).toBe("From v0.3.0. The other machine has v0.3.0.");
    expect(updatedResult({ ...done, peer: null })?.body).toBe("From v0.3.0.");
    expect(updatedResult({ ...done, paired: false, peer: null })?.body).toBe("From v0.3.0.");
  });
});
