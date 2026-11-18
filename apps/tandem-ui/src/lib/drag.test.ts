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

describe("aviso del arrastre de archivos", () => {
  it("dice qué archivo se copia y cuántos faltan", () => {
    expect(dragNotice({ kind: "copying", name: "a.txt", index: 1, total: 3 })).toEqual({ text: "Copiando 2 de 3: a.txt", tone: "ok", ongoing: true });
    expect(dragNotice({ kind: "copied", total: 1 }).text).toBe("Listo para soltar: 1 file");
    expect(dragNotice({ kind: "copied", total: 2 }).text).toBe("Listo para soltar: 2 archivos");
  });

  it("los finales cierran el aviso y un fallo va en tono de error", () => {
    expect(dragNotice({ kind: "dropped" }).ongoing).toBe(false);
    expect(dragNotice({ kind: "cancelled" }).ongoing).toBe(false);
    expect(dragNotice({ kind: "failed", reason: "sin espacio" })).toEqual({ text: "No se pudo arrastrar: sin espacio", tone: "bad", ongoing: false });
  });

  it("LiveSource reparte el aviso con el mismo oyente de avisos que las transacciones", async () => {
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

  it("el store usa un solo aviso que se actualiza y se cierra solo al terminar", async () => {
    vi.useFakeTimers();
    const { src, emit } = fake();
    const store = new Store(src);
    await store.start();
    emit({ kind: "copying", name: "a.txt", index: 0, total: 2 });
    emit({ kind: "copying", name: "b.txt", index: 1, total: 2 });
    expect(store.toasts.map((t) => t.text)).toEqual(["Copiando 2 de 2: b.txt"]);
    // Mientras dura el arrastre no se cierra solo.
    vi.advanceTimersByTime(30_000);
    expect(store.toasts).toHaveLength(1);
    emit({ kind: "copied", total: 2 });
    emit({ kind: "dropped" });
    expect(store.toasts.map((t) => t.text)).toEqual(["Files copied to the other machine"]);
    vi.advanceTimersByTime(TOAST_MS);
    expect(store.toasts).toHaveLength(0);
  });

  it("un fallo sin avance previo igual se avisa", async () => {
    const { src, emit } = fake();
    const store = new Store(src);
    await store.start();
    emit({ kind: "failed", reason: "sin espacio" });
    expect(store.toasts[0]).toMatchObject({ tone: "bad" });
  });
});
