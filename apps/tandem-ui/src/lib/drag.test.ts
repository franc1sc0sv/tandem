import { afterEach, describe, expect, it, vi } from "vitest";

import type { Bridge } from "./bridge";
import { dragNotice } from "./drag";
import type { DragStatus, Notice, Snapshot } from "./model";
import { LiveSource } from "./store/live";
import { Store, TOAST_MS } from "./store/store.svelte";

afterEach(() => vi.useRealTimers());

function fake() {
  let notice: ((n: Notice) => void) | undefined;
  const onNotice = vi.fn(async (cb: (n: Notice) => void) => {
    notice = cb;
  });
  const bridge = { snapshot: async () => ({ kind: "connecting" }) as Snapshot, onSnapshot: async () => {}, act: vi.fn(async () => ({ kind: "done" })), onNotice } as unknown as Bridge;
  return { src: new LiveSource(bridge), onNotice, emit: (status: DragStatus) => notice?.({ kind: "drag", status }) };
}

describe("file drag notice", () => {
  it("says which file is being copied and how many are left", () => {
    expect(dragNotice({ kind: "copying", name: "a.txt", index: 1, total: 3 })).toEqual({ text: "Copying 2 of 3: a.txt", tone: "ok", ongoing: true });
    expect(dragNotice({ kind: "copied", total: 1 }).text).toBe("Ready to drop: 1 file");
    expect(dragNotice({ kind: "copied", total: 2 }).text).toBe("Ready to drop: 2 files");
  });

  it("the final ones close the notice and a failure uses the error tone", () => {
    expect(dragNotice({ kind: "dropped" }).ongoing).toBe(false);
    expect(dragNotice({ kind: "cancelled" }).ongoing).toBe(false);
    expect(dragNotice({ kind: "failed", reason: "no space" })).toEqual({ text: "Could not drag: no space", tone: "bad", ongoing: false });
  });

  it("LiveSource delivers the notice with the same notice listener as the transactions", async () => {
    const { src, onNotice, emit } = fake();
    const got: DragStatus[] = [];
    const stop = await src.watchDrag((s) => got.push(s));
    await src.watchTxn(() => {});
    expect(onNotice).toHaveBeenCalledTimes(1);
    emit({ kind: "dropped" });
    stop();
    emit({ kind: "cancelled" });
    expect(got).toEqual([{ kind: "dropped" }]);
  });

  it("the store uses a single notice that updates and closes by itself at the end", async () => {
    vi.useFakeTimers();
    const { src, emit } = fake();
    const store = new Store(src);
    await store.start();
    emit({ kind: "copying", name: "a.txt", index: 0, total: 2 });
    emit({ kind: "copying", name: "b.txt", index: 1, total: 2 });
    expect(store.toasts.map((t) => t.text)).toEqual(["Copying 2 of 2: b.txt"]);
    // While the drag lasts it does not close by itself.
    vi.advanceTimersByTime(30_000);
    expect(store.toasts).toHaveLength(1);
    emit({ kind: "copied", total: 2 });
    emit({ kind: "dropped" });
    expect(store.toasts.map((t) => t.text)).toEqual(["Files copied to the other machine"]);
    vi.advanceTimersByTime(TOAST_MS);
    expect(store.toasts).toHaveLength(0);
  });

  it("a failure without prior progress is still announced", async () => {
    const { src, emit } = fake();
    const store = new Store(src);
    await store.start();
    emit({ kind: "failed", reason: "no space" });
    expect(store.toasts[0]).toMatchObject({ tone: "bad" });
  });
});
