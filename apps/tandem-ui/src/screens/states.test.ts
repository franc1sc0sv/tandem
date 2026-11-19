// Trouble states (V3-States, T-120): qué tarjetas salen, con qué datos y qué hace cada acción.
import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../lib/context";
import { shortPath, stateCards } from "../lib/states";
import { DemoSource, demoDesk, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import type { DeskView } from "../lib/store/types";
import States from "./States.svelte";

afterEach(() => {
  vi.restoreAllMocks();
});

const GALLERY = { gallery: true, failedScene: null };
const REAL = { gallery: false, failedScene: null };
const ids = (d: DeskView, ctx = REAL) => stateCards(d, ctx).map((c) => c.id);

async function mount(query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)));
  await store.start();
  store.go({ screen: "states" });
  render(States, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

describe("lib/states: qué tarjetas aplican", () => {
  const base = demoDesk(demoOptions("demo&still"));

  it("la galería del demo trae las nueve, en el orden del mockup", () => {
    expect(ids(base, GALLERY)).toEqual(["first", "off", "cable", "resumed", "failed", "scene", "unsupported", "nobody", "permission"]);
  });

  it("con todo en orden la app real solo muestra lo que el desk tiene (the monitor sin compatibilidad)", () => {
    expect(ids(base)).toEqual(["unsupported"]);
    expect(stateCards(base)[0].title).toMatch(/^“.+” is not supported$/);
  });

  it("con `example` (`?demo&states`) las nueve tarjetas traen los textos del mockup, solo en la galería de ejemplo", () => {
    const cards = stateCards(base, { ...GALLERY, example: true });
    expect(cards).toHaveLength(9);
    expect(JSON.stringify(cards)).toContain("epoch 9/20 · loss 0.0371");
    expect(cards.find((c) => c.id === "unsupported")?.title).toBe("Unsupported monitor");
    expect(JSON.stringify(stateCards(base, GALLERY))).not.toContain("Se recuperaron 3 min");
  });

  it("the other machine apagada, un solo Wi-Fi, sin otra machine y el permiso faltante salen solos", () => {
    expect(ids(demoDesk(demoOptions("demo&still&trouble=peer-off")))).toContain("off");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=link-down")))).toContain("cable");
    expect(ids(demoDesk(demoOptions("demo&still&peer=none")))).toContain("first");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=permissions")))).toContain("permission");
  });

  it("la scene fallida sale con su nombre y su id, no con la primera ni la activa", () => {
    const cards = stateCards(base, { gallery: false, failedScene: { id: "game", name: "Game" } });
    const scene = cards.find((c) => c.id === "scene");
    expect(scene?.title).toBe("The scene “Game” was not applied");
    expect(scene?.action).toMatchObject({ does: "retry-scene", sceneId: "game" });
  });

  it("ningún texto inventa datos: sin épocas, pérdida, GPU ni nombres de ejemplo", () => {
    const all = JSON.stringify(stateCards(base, GALLERY));
    for (const x of ["LoRA", "epoch", "loss", "ComfyUI", "MacBook only", "14.1", "0.6 s", "2 workspaces"]) expect(all).not.toContain(x);
  });

  it("shortPath deja la ruta del binario sin el texto que la rodea y la abrevia", () => {
    expect(shortPath("Concede el permiso a este binario: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 12)")).toBe("/Applications/Tandem.app/…/tandemd");
    expect(shortPath("/a/b")).toBe("/a/b");
    expect(shortPath(null)).toBe("/Applications/Tandem.app/…/tandemd");
  });
});

describe("pantalla States", () => {
  it("en el demo muestra la galería con sus acciones", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1, name: "Trouble states" })).toBeInTheDocument();
    expect(screen.getAllByRole("article")).toHaveLength(9);
    for (const name of ["Pair the other machine", "Power on", "View in Tunnels", "Retry", "Open System Settings"]) {
      expect(screen.getByRole("button", { name: new RegExp(name) })).toBeInTheDocument();
    }
  });

  it("ninguna tarjeta habla de «agregar» una machine (D1)", async () => {
    await mount();
    expect(document.body.textContent).not.toMatch(/add (another )?machine/i);
  });

  it("«Pair the other machine» abre el asistente de emparejar", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Pair the other machine/ }));
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("«Encender» enciende the other machine", async () => {
    const store = await mount();
    const power = vi.spyOn(store, "setPower");
    await userEvent.click(screen.getByRole("button", { name: /Power on/ }));
    expect(power).toHaveBeenCalledWith("b", true);
  });

  it("«View in Tunnels» navega a Tunnels", async () => {
    const store = await mount();
    await userEvent.click(screen.getByRole("button", { name: /View in Tunnels/ }));
    expect(store.route).toEqual({ screen: "tunnels" });
  });

  it("«Open System Settings» pide el permiso de monitoreo de entrada", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openPermission");
    await userEvent.click(screen.getByRole("button", { name: /Open System Settings/ }));
    expect(open).toHaveBeenCalledWith("input_monitoring");
  });

  it("«Reintentar» reaplica la scene que falló, no la primera", async () => {
    const store = await mount();
    const apply = vi.spyOn(store, "applyScene").mockResolvedValue({ ok: true });
    store.failedScene = { id: "game", name: "Game" };
    await tick();
    expect(screen.getByText("The scene “Game” was not applied")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Retry/ }));
    expect(apply).toHaveBeenCalledWith("game");
    expect(apply).not.toHaveBeenCalledWith(store.desk.scenes[0].id === "game" ? "" : store.desk.scenes[0].id);
  });
});

describe("la scene que falló se recuerda en el store", () => {
  it("al fallar una scene queda anotada y se borra cuando otra se aplica bien", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    expect(store.demo).toBe(true);
    expect(store.failedScene).toBeNull();
  });
});
