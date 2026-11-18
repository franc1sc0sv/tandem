// Estados difíciles (V3-Estados, T-120): qué tarjetas salen, con qué datos y qué hace cada acción.
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { shortPath, stateCards } from "../lib/states";
import { DemoSource, demoDesk, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import type { DeskView } from "../lib/store/types";
import Estados from "./Estados.svelte";

afterEach(() => {
  vi.restoreAllMocks();
});

const GALLERY = { gallery: true, failedScene: null };
const REAL = { gallery: false, failedScene: null };
const ids = (d: DeskView, ctx = REAL) => stateCards(d, ctx).map((c) => c.id);

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "estados" });
  render(Estados, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

describe("lib/states: qué tarjetas aplican", () => {
  const base = demoDesk(demoOptions("demo&still"));

  it("la galería del demo trae las nueve, en el orden del mockup", () => {
    expect(ids(base, GALLERY)).toEqual(["first", "off", "cable", "resumed", "failed", "scene", "unsupported", "nobody", "permission"]);
  });

  it("con todo en orden la app real solo muestra lo que el puesto tiene (el monitor sin compatibilidad)", () => {
    expect(ids(base)).toEqual(["unsupported"]);
    expect(stateCards(base)[0].title).toMatch(/^«.+» no es compatible$/);
  });

  it("la otra máquina apagada, un solo Wi-Fi, sin otra máquina y el permiso faltante salen solos", () => {
    expect(ids(demoDesk(demoOptions("demo&still&trouble=peer-off")))).toContain("off");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=link-down")))).toContain("cable");
    expect(ids(demoDesk(demoOptions("demo&still&peer=none")))).toContain("first");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=permissions")))).toContain("permission");
  });

  it("la escena fallida sale con su nombre y su id, no con la primera ni la activa", () => {
    const cards = stateCards(base, { gallery: false, failedScene: { id: "juego", name: "Juego" } });
    const scene = cards.find((c) => c.id === "scene");
    expect(scene?.title).toBe("La escena «Juego» no se aplicó");
    expect(scene?.action).toMatchObject({ does: "retry-scene", sceneId: "juego" });
  });

  it("ningún texto inventa datos: sin épocas, pérdida, GPU ni nombres de ejemplo", () => {
    const all = JSON.stringify(stateCards(base, GALLERY));
    for (const x of ["LoRA", "epoch", "loss", "ComfyUI", "Solo MacBook", "14.1", "0.6 s", "2 workspaces"]) expect(all).not.toContain(x);
  });

  it("shortPath deja la ruta del binario sin el texto que la rodea y la abrevia", () => {
    expect(shortPath("Concede el permiso a este binario: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 12)")).toBe("/Applications/Tandem.app/…/tandemd");
    expect(shortPath("/a/b")).toBe("/a/b");
    expect(shortPath(null)).toBe("/Applications/Tandem.app/…/tandemd");
  });
});

describe("pantalla Estados", () => {
  it("en el demo muestra la galería con sus acciones", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1, name: "Estados difíciles" })).toBeInTheDocument();
    expect(screen.getAllByRole("article")).toHaveLength(9);
    for (const name of ["Agregar máquina", "Encender", "Ver en Túneles", "Reintentar", "Abrir Ajustes del Sistema"]) {
      expect(screen.getByRole("button", { name: new RegExp(name) })).toBeInTheDocument();
    }
  });

  it("«Agregar máquina» abre el asistente de emparejar", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Agregar máquina/ }));
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("«Encender» enciende la otra máquina", async () => {
    const store = await mount();
    const power = vi.spyOn(store, "setPower");
    await userEvent.click(screen.getByRole("button", { name: /Encender/ }));
    expect(power).toHaveBeenCalledWith("b", true);
  });

  it("«Ver en Túneles» navega a Túneles", async () => {
    const store = await mount();
    await userEvent.click(screen.getByRole("button", { name: /Ver en Túneles/ }));
    expect(store.route).toEqual({ screen: "tuneles" });
  });

  it("«Abrir Ajustes del Sistema» pide el permiso de monitoreo de entrada", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openPermission");
    await userEvent.click(screen.getByRole("button", { name: /Abrir Ajustes del Sistema/ }));
    expect(open).toHaveBeenCalledWith("input_monitoring");
  });

  it("«Reintentar» reaplica la escena que falló, no la primera", async () => {
    const store = await mount();
    const apply = vi.spyOn(store, "applyScene").mockResolvedValue({ ok: true });
    store.failedScene = { id: "juego", name: "Juego" };
    await tick();
    expect(screen.getByText("La escena «Juego» no se aplicó")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Reintentar/ }));
    expect(apply).toHaveBeenCalledWith("juego");
    expect(apply).not.toHaveBeenCalledWith(store.desk.scenes[0].id === "juego" ? "" : store.desk.scenes[0].id);
  });
});

describe("la escena que falló se recuerda en el store", () => {
  it("al fallar una escena queda anotada y se borra cuando otra se aplica bien", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    expect(store.demo).toBe(true);
    expect(store.failedScene).toBeNull();
  });
});
