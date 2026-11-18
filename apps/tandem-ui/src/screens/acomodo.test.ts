import { readFileSync } from "node:fs";
import { join } from "node:path";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it, vi } from "vitest";

import type { Bridge } from "../lib/bridge";
import { formatHash, parseHash } from "../lib/routes";
import { STORE_KEY } from "../lib/context";
import type { Action, Notice, Snapshot } from "../lib/model";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { LiveSource, layoutFromDesk } from "../lib/store/live";
import { Store } from "../lib/store/store.svelte";
import type { Source } from "../lib/store/types";
import Shell from "../shell/Shell.svelte";

async function open(source: Source, hash = "#/acomodo") {
  const store = new Store(source, { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const pad = () => screen.getByRole("radiogroup", { name: /¿Dónde está/ });
const align = () => screen.getByRole("radiogroup", { name: "Alineación" });
const slot = (name: string) => within(pad()).getByRole("radio", { name });
const alignTo = (name: string) => within(align()).getByRole("radio", { name });

describe("Acomodo (V3-Acomodo) en modo demo", () => {
  it("abre con el 34\" elegido, la otra máquina a la izquierda y alineada abajo", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    expect(screen.getByRole("button", { name: /Odyssey G5 34"/ })).toHaveAttribute("aria-pressed", "true");
    expect(pad()).toHaveAccessibleName("¿Dónde está MacBook Pro?");
    expect(slot("Izquierda")).toHaveAttribute("aria-checked", "true");
    const opts = [...align().querySelectorAll("[role=radio]")].map((b) => b.textContent);
    expect(opts).toEqual(["Arriba", "Centro", "Abajo"]);
    expect(alignTo("Abajo")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByText("Borde de cruce")).toBeInTheDocument();
    expect(screen.getByText("Trabajo")).toBeInTheDocument();
    // Sin X/Y (D18).
    expect(screen.queryByRole("spinbutton")).toBeNull();
    expect(screen.queryByText(/Avanzado/)).toBeNull();
  });

  it("mover la otra máquina arriba cambia las etiquetas de la alineación y el borde pasa a horizontal", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    const edge = () => document.querySelector<HTMLElement>(".xedge");
    expect(edge()?.style.width).toBe("3px");
    await userEvent.click(slot("Arriba"));
    expect(slot("Arriba")).toHaveAttribute("aria-checked", "true");
    expect(slot("Izquierda")).toHaveAttribute("aria-checked", "false");
    const opts = [...align().querySelectorAll("[role=radio]")].map((b) => b.textContent);
    expect(opts).toEqual(["Izquierda", "Centro", "Derecha"]);
    expect(edge()?.style.height).toBe("3px");
  });

  it("guardar manda el acomodo al puesto y avisa", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    const store = await open(src);
    await userEvent.click(slot("Abajo"));
    await userEvent.click(alignTo("Centro"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    expect(store.desk.layout.arrangement).toEqual({ aSide: "down", align: "center" });
    expect(await screen.findByText("Acomodo guardado")).toBeInTheDocument();
  });

  it("con la MacBook elegida el mini plano se ve desde ella", async () => {
    const store = await open(new DemoSource(demoOptions("demo&still")));
    await userEvent.click(screen.getByRole("button", { name: /MacBook Pro/ }));
    expect(pad()).toHaveAccessibleName('¿Dónde está el 34"?');
    expect(slot("Derecha")).toHaveAttribute("aria-checked", "true");
    // Poner el 34" arriba de la MacBook es tener la MacBook abajo del 34".
    await userEvent.click(slot("Arriba"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    expect(store.desk.layout.arrangement.aSide).toBe("down");
  });

  it("una pantalla desconectada dice que no entra en el acomodo", async () => {
    await open(new DemoSource(demoOptions("demo&still")));
    await userEvent.click(screen.getByRole("button", { name: /Odyssey G5 32"/ }));
    expect(screen.getByText("No compatible: no entra en el acomodo.")).toBeInTheDocument();
    expect(screen.queryByRole("radiogroup", { name: "Alineación" })).toBeNull();
    expect(screen.getByText("Desconectado")).toBeInTheDocument();
  });

  it("las animaciones del mockup están y son de CSS (las apaga prefers-reduced-motion)", () => {
    const src = readFileSync(join(process.cwd(), "src", "screens", "Acomodo.svelte"), "utf8");
    for (const k of ["@keyframes gl ", "@keyframes gl2", "@keyframes fd", "cubic-bezier(0.3, 1.2, 0.5, 1)", "cubic-bezier(0.3, 1.3, 0.5, 1)", "cubic-bezier(0.3, 1.5, 0.5, 1)"]) expect(src).toContain(k);
    const cross = readFileSync(join(process.cwd(), "src", "screens", "CrossTest.svelte"), "utf8");
    for (const k of ["@keyframes eg", "@keyframes nud", "@keyframes cnt", "@keyframes bs", "@keyframes in"]) expect(cross).toContain(k);
  });
});

describe("Prueba de cruce (F-PruebaCruce)", () => {
  it("cuelga del acomodo: `#/acomodo/cruce`", () => {
    expect(parseHash("#/acomodo/cruce")).toEqual({ screen: "cruce" });
    expect(formatHash({ screen: "cruce" })).toBe("#/acomodo/cruce");
    expect(parseHash("#/acomodo/otra")).toEqual({ screen: "acomodo" });
  });


  it("pide cruzar por el borde del acomodo y pasa con el cruce", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await open(src, "#/acomodo/cruce");
    expect(screen.getByText("Lleva el mouse a la derecha de la MacBook")).toBeInTheDocument();
    expect(screen.getByText("esperando el cursor")).toBeInTheDocument();
    src.cross({ from: "a", to: "b", edge: "right", at: 612, of: 1440, ms: 0.4 });
    expect(await screen.findByText("Cruzó bien")).toBeInTheDocument();
    expect(screen.getByText(/Entró a PC-Escritorio a la altura correcta \(y = 612 de 1440\)\. Latencia del cruce: 0\.4 ms\./)).toBeInTheDocument();
    expect(screen.getByText("el cursor llegó aquí")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Probar otra vez/ }));
    expect(await screen.findByText("Lleva el mouse a la derecha de la MacBook")).toBeInTheDocument();
  });

  it("un cruce por otro borde dice por cuál y ofrece editar el acomodo", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await open(src, "#/acomodo/cruce");
    src.cross({ from: "a", to: "b", edge: "up", at: 10, of: 3440, ms: 1 });
    expect(await screen.findByText("Cruzó por otro borde")).toBeInTheDocument();
    expect(screen.getByText(/Cruzó por arriba, no por la derecha/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /Editar acomodo/ })).toHaveAttribute("href", "#/acomodo");
  });

  it("sin cruce a tiempo se agota", async () => {
    await open(new DemoSource(demoOptions("demo&still&cross=timeout")), "#/acomodo/cruce");
    expect(await screen.findByText("No cruzó en 20 s")).toBeInTheDocument();
    expect(screen.getByText(/Quizá el 34" está en otro lado de tu escritorio/)).toBeInTheDocument();
  });

  it("el borde pedido sigue al acomodo guardado", async () => {
    const src = new DemoSource(demoOptions("demo&still"));
    await src.setArrangement({ aSide: "down", align: "start" });
    await open(src, "#/acomodo/cruce");
    // A está abajo de B: desde A se sale por arriba.
    expect(screen.getByText("Lleva el mouse arriba de la MacBook")).toBeInTheDocument();
  });
});

describe("con el agente real", () => {
  const connected = (): Snapshot => ({
    kind: "connected",
    agent: "a",
    desk: {
      machines: { a: { os: "macos", name: "MacBook Pro", color: "#7aa7e6" }, b: { os: "windows", name: "PC-Escritorio", color: "#a99be8" } },
      screens: [
        { machine: "a", id: "Integrada", x: 0, y: 0, width: 1512, height: 945, scale: 2 },
        { machine: "b", id: "Odyssey", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
      ],
      arrangement: { a_side: "right", align: "start" },
      scenes: [],
    },
    state: { on: { a: true, b: true } },
    peer: { kind: "paired", name: "PC-Escritorio", link: "usb4" },
    pairing: { kind: "idle" },
    input: { server: "a", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: true },
    caps: {
      here: {
        machine: "a",
        name: "MacBook Pro",
        os: "macos",
        color: "#7aa7e6",
        hub: null,
        input: { capture: true, inject: true, missing: [], process: { exe: "", pid: 1, started: 1 } },
        power: { wake_peer: true, wakeable: true, power_off: true },
        services: [],
      },
      peer: null,
    },
    monitors: [],
  });

  /** Lo que la prueba no usa se queda esperando. */
  function bridge(act: (a: Action) => Promise<unknown>): Bridge {
    const known: Partial<Record<keyof Bridge, unknown>> = { snapshot: async () => connected(), onSnapshot: async () => {}, act };
    return new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
  }

  it("las pantallas y el acomodo salen del puesto", () => {
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("foto");
    const l = layoutFromDesk(snap);
    expect(l.arrangement).toEqual({ aSide: "right", align: "start" });
    expect(l.screens.map((s) => [s.id, s.machine, s.res, s.ratio, s.scale])).toEqual([
      ["a:Integrada", "a", "1512 × 945", "16:10", "200 %"],
      ["b:Odyssey", "b", "3440 × 1440", "21:9", "100 %"],
    ]);
    // Sin pantallas declaradas: una por máquina, con su nombre.
    const none = layoutFromDesk({ ...snap, desk: { ...snap.desk, screens: [], arrangement: null } });
    expect(none.screens.map((s) => s.name)).toEqual(["MacBook Pro", "PC-Escritorio"]);
    expect(none.arrangement).toEqual({ aSide: "left", align: "center" });
  });

  /** El puesto con la PC (b) con dos monitores conectados, uno al lado del otro. */
  const twoScreens = (): Snapshot => {
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("foto");
    return {
      ...snap,
      desk: {
        ...snap.desk,
        screens: [
          { machine: "a", id: "Integrada", x: 0, y: 0, width: 1512, height: 945, scale: 2 },
          { machine: "b", id: "Odyssey", x: 0, y: 0, width: 3440, height: 1440, scale: 1 },
          { machine: "b", id: "Lateral", x: 3440, y: -200, width: 2560, height: 1440, scale: 1 },
        ],
      },
    };
  };

  it("las pantallas de una máquina llevan su posición en el escritorio", () => {
    const snap = twoScreens();
    if (snap.kind !== "connected") throw new Error("foto");
    const l = layoutFromDesk(snap);
    const lateral = l.screens.find((s) => s.id === "b:Lateral");
    const main = l.screens.find((s) => s.id === "b:Odyssey");
    expect(lateral?.usable).toBe(true);
    expect(lateral && main && lateral.x - main.x).toBeCloseTo(3440 / 110, 9);
    expect(lateral?.y).toBeCloseTo(-200 / 110, 9);
  });

  it("una segunda pantalla conectada no es «No compatible» y edita el mismo acomodo", async () => {
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const bridgeTwo: Bridge = new Proxy({} as Bridge, {
      get: (_t, k) => ({ snapshot: async () => twoScreens(), onSnapshot: async () => {}, act })[k as string] ?? (() => new Promise(() => {})),
    });
    await open(new LiveSource(bridgeTwo));
    await userEvent.click(screen.getByRole("button", { name: /Lateral/ }));
    expect(screen.queryByText(/No compatible/)).toBeNull();
    expect(pad()).toHaveAccessibleName("¿Dónde está Integrada?");
    await userEvent.click(slot("Arriba"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    // Desde b la MacBook está arriba: se guarda como el lado de A.
    expect(act).toHaveBeenCalledWith({ kind: "set_arrangement", a_side: "up", align: "start" });
  });

  it("al guardar el lienzo no vuelve al acomodo viejo mientras llega el nuevo", async () => {
    let push: (s: Snapshot) => void = () => {};
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const known: Partial<Record<keyof Bridge, unknown>> = {
      snapshot: async () => connected(),
      onSnapshot: async (cb: (s: Snapshot) => void) => {
        push = cb;
      },
      act,
    };
    const b = new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
    await open(new LiveSource(b));
    await userEvent.click(slot("Abajo"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    expect(await screen.findByText("Acomodo guardado")).toBeInTheDocument();
    // El agente respondió, pero el `Desk` nuevo no llegó: sigue viéndose lo guardado.
    expect(slot("Abajo")).toHaveAttribute("aria-checked", "true");
    const snap = connected();
    if (snap.kind !== "connected") throw new Error("foto");
    // Una foto que repite el acomodo viejo no suelta el borrador.
    push(snap);
    await tick();
    expect(slot("Abajo")).toHaveAttribute("aria-checked", "true");
    push({ ...snap, desk: { ...snap.desk, arrangement: { a_side: "down", align: "start" } } });
    await tick();
    expect(slot("Abajo")).toHaveAttribute("aria-checked", "true");
    // Con el acomodo nuevo ya en el puesto el borrador se soltó: lo que llegue después se ve.
    push({ ...snap, desk: { ...snap.desk, arrangement: { a_side: "left", align: "end" } } });
    await tick();
    expect(slot("Izquierda")).toHaveAttribute("aria-checked", "true");
  });

  it("guardar es un cambio del puesto por la IPC (`set_arrangement`)", async () => {
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    await open(new LiveSource(bridge(act)));
    await userEvent.click(slot("Abajo"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    expect(act).toHaveBeenCalledWith({ kind: "set_arrangement", a_side: "down", align: "start" });
    expect(await screen.findByText("Acomodo guardado")).toBeInTheDocument();
  });

  it("si el agente rechaza, lo dice y el cambio queda sin guardar", async () => {
    const act = vi.fn(async () => ({ kind: "rejected" as const, reason: "la máquina b todavía no se conoce" }));
    const store = await open(new LiveSource(bridge(act)));
    await userEvent.click(slot("Abajo"));
    await userEvent.click(screen.getByRole("button", { name: /Guardar/ }));
    expect(await screen.findByText("la máquina b todavía no se conoce")).toBeInTheDocument();
    expect(slot("Abajo")).toHaveAttribute("aria-checked", "true");
    expect(store.desk.layout.arrangement.aSide).toBe("right");
  });

  it("la prueba de cruce usa el cruce que informa el agente: botón habilitado y «Cruzó bien» por el borde correcto", async () => {
    let notice: ((n: Notice) => void) | undefined;
    const act = vi.fn(async (_a: Action) => ({ kind: "done" as const }));
    const withNotice = (): Bridge => {
      const known: Partial<Record<keyof Bridge, unknown>> = {
        snapshot: async () => connected(),
        onSnapshot: async () => {},
        act,
        onNotice: async (cb: (n: Notice) => void) => {
          notice = cb;
        },
      };
      return new Proxy({} as Bridge, { get: (_t, k) => known[k as keyof Bridge] ?? (() => new Promise(() => {})) });
    };
    await open(new LiveSource(withNotice()));
    expect(screen.getByRole("link", { name: /Probar cruce/ })).toBeInTheDocument();
    document.body.innerHTML = "";
    await open(new LiveSource(withNotice()), "#/acomodo/cruce");
    await waitFor(() => expect(act).toHaveBeenCalledWith({ kind: "watch_cursor", on: true }));
    // El acomodo dice «A a la derecha»: la Mac sale por su borde izquierdo.
    notice?.({ kind: "crossed", from: "a", to: "b", edge: "left" });
    notice?.({ kind: "cursor", sample: { machine: "b", screen: "Odyssey", x: 3439, y: 700 } });
    expect(await screen.findByText("Cruzó bien")).toBeInTheDocument();
  });
});
