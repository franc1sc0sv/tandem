import { describe, expect, it } from "vitest";

import { CROSS_TIMEOUT_MS, formatMs, onSample, onTick, remaining, start, type CrossSample } from "./crossing";

const sample = (over: Partial<CrossSample> = {}): CrossSample => ({ from: "a", to: "b", edge: "right", at: 612, of: 1440, ms: 0.4, ...over });

describe("crossing test", () => {
  it("passes with a crossing through the arrangement edge", () => {
    const p = onSample(start(0), "a", "right", sample());
    expect(p).toEqual({ kind: "passed", sample: sample() });
  });

  it("a crossing through another edge fails with that edge", () => {
    const p = onSample(start(0), "a", "right", sample({ edge: "up" }));
    expect(p.kind).toBe("wrong-edge");
    if (p.kind === "wrong-edge") expect(p.sample.edge).toBe("up");
  });

  it("a crossing back (from the other one) does not count, and nothing changes the result already given", () => {
    const waiting = start(0);
    expect(onSample(waiting, "a", "right", sample({ from: "b", to: "a", edge: "left" }))).toBe(waiting);
    const passed = onSample(waiting, "a", "right", sample());
    expect(onSample(passed, "a", "right", sample({ edge: "up" }))).toBe(passed);
    expect(onTick(passed, CROSS_TIMEOUT_MS * 2)).toBe(passed);
  });

  it("without a crossing in 20 s it times out", () => {
    const w = start(1000);
    expect(onTick(w, 1000 + CROSS_TIMEOUT_MS - 1)).toBe(w);
    expect(onTick(w, 1000 + CROSS_TIMEOUT_MS)).toEqual({ kind: "timeout" });
    expect(onTick(w, 1000, 0)).toEqual({ kind: "timeout" });
    expect(remaining(w, 1000)).toBe(20);
    expect(remaining(w, 1000 + 19_001)).toBe(1);
    expect(remaining({ kind: "timeout" }, 0)).toBe(0);
  });

  it("latency is shown with one decimal if small", () => {
    expect(formatMs(0.4)).toBe("0.4 ms");
    expect(formatMs(3)).toBe("3.0 ms");
    expect(formatMs(12.6)).toBe("13 ms");
  });
});
