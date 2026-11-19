import { describe, expect, it } from "vitest";

import type { Step, TxnView } from "./model";
import type { MonitorUi } from "./store/types";
import { feedRun, running, titleOf, viewOf, type Names, type Run } from "./txn";

const names: Names = {
  machine: (m) => (m === "a" ? "MacBook Pro" : "PC-Desktop"),
  monitors: [{ id: "g5-34", name: 'Odyssey G5 34"', compat: { ok: true }, connected: true, owner: "a", input: "hdmi", ports: { a: "hdmi", b: "display_port" }, shows: "a", events: [] } satisfies MonitorUi],
};
const steps: Step[] = [
  { step: "show_machine", monitor: "g5-34", machine: "b" },
  { step: "geometry" },
];
const origin = { title: "Applying the scene “Game”", doneText: "Scene “Game” active." };

function play(events: TxnView[], at = 1000): Run {
  let run: Run | null = null;
  events.forEach((e, i) => {
    run = feedRun(run, e, at + i * 600, origin);
  });
  if (!run) throw new Error("no run");
  return run;
}
const started: TxnView = { type: "started", txn: 7, steps };

describe("transaction progress (F-Transactions)", () => {
  it("starts with \"Save how everything is\" ready and the steps pending", () => {
    const run = play([started]);
    expect(running(run)).toBe(true);
    const v = viewOf(run, names, 1000);
    expect(v.title).toBe("Applying the scene “Game”");
    expect(v.rows.map((r) => [r.icon, r.text])).toEqual([
      ["done", "Save how everything is"],
      ["todo", '34" → PC-Desktop (DP)'],
      ["todo", 'Mouse: take the 34" off MacBook Pro'],
    ]);
    expect(v.message).toEqual({ text: "The monitor confirms each change; nothing is guessed.", tone: "mut" });
  });

  it("the monitor step waits for the monitor and when it ends says how long it took", () => {
    const run = play([started, { type: "step_started", txn: 7, index: 0 }]);
    expect(viewOf(run, names, 2000).rows[1]).toMatchObject({ icon: "run", detail: "waiting for the monitor" });
    const done = play([started, { type: "step_started", txn: 7, index: 0 }, { type: "step_done", txn: 7, index: 0 }]);
    expect(viewOf(done, names, 3000).rows[1]).toMatchObject({ icon: "done", detail: "confirmed · 0.6 s" });
  });

  it("on confirmation it says what the requester asked for and stops running", () => {
    const run = play([started, { type: "step_started", txn: 7, index: 0 }, { type: "step_done", txn: 7, index: 0 }, { type: "step_started", txn: 7, index: 1 }, { type: "step_done", txn: 7, index: 1 }, { type: "committed", txn: 7, state: { on: { a: true, b: true } } }]);
    expect(running(run)).toBe(false);
    const v = viewOf(run, names, 9000);
    expect(v.outcome).toBe("ok");
    expect(v.message).toEqual({ text: "Scene “Game” active.", tone: "ok" });
    expect(v.elapsed).toBe("3.0 s");
  });

  it("if the monitor gives no image, the rollback is visible and the message says what happened", () => {
    const run = play([
      started,
      { type: "step_started", txn: 7, index: 0 },
      { type: "step_failed", txn: 7, index: 0, error: "the monitor showed no image on DisplayPort" },
    ]);
    const v = viewOf(run, names, 5000);
    expect(v.rows.map((r) => r.icon)).toEqual(["done", "bad", "back"]);
    expect(v.rows[1]).toMatchObject({ tone: "bad", detail: "the monitor said: change failed" });
    expect(v.rows[2]).toMatchObject({ text: "Rolling back to how it was", detail: '34" → MacBook Pro' });
    expect(v.message).toEqual({ text: "There was no DisplayPort signal. Everything goes back to how it was; nothing was left half done.", tone: "warn" });
    const over = play([started, { type: "step_started", txn: 7, index: 0 }, { type: "step_failed", txn: 7, index: 0, error: "the monitor showed no image on DisplayPort" }, { type: "failed", txn: 7, state: { on: { a: true, b: true } } }]);
    expect(viewOf(over, names, 5000)).toMatchObject({ outcome: "failed" });
    expect(viewOf(over, names, 5000).rows[2]?.icon).toBe("done");
  });

  it("notices of another transaction do not move the run", () => {
    const run = play([started]);
    expect(feedRun(run, { type: "step_started", txn: 99, index: 0 }, 5000)).toBe(run);
    expect(feedRun(null, { type: "committed", txn: 7, state: { on: { a: true, b: true } } }, 5000)).toBeNull();
  });

  it("without its own title it is described by the steps", () => {
    expect(titleOf([{ step: "power_on", machine: "b" }], names)).toBe("Powering on PC-Desktop");
    expect(titleOf([{ step: "power_off", machine: "b" }], names)).toBe("Powering off PC-Desktop");
    expect(titleOf([{ step: "switch_monitor", monitor: "g5-34", input: "hdmi" }], names)).toBe("Changing the input of 34\"");
    expect(titleOf(steps, names)).toBe("Applying a scene");
  });
});
