import { describe, expect, it } from "vitest";
import { availableRow, checkLabel, upToDate, versionRow, type UpdateView } from "./updates";

const base: UpdateView = { here: "0.3.0", peer: "0.3.0", paired: true, available: null, checking: false, step: null, error: null, auto: true };

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
