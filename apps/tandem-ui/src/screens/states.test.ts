// Trouble states (V3-States, T-120): which cards appear, with what data and what each action does.
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

describe("lib/states: which cards apply", () => {
  const base = demoDesk(demoOptions("demo&still"));

  it("the demo gallery brings all nine, in the mockup order", () => {
    expect(ids(base, GALLERY)).toEqual(["first", "off", "cable", "resumed", "failed", "scene", "unsupported", "nobody", "permission"]);
  });

  it("with everything in order the real app only shows what the desk has (the monitor without compatibility)", () => {
    expect(ids(base)).toEqual(["unsupported"]);
    expect(stateCards(base)[0].title).toMatch(/^“.+” is not supported$/);
  });

  it("with `example` (`?demo&states`) the nine cards carry the mockup texts, only in the example gallery", () => {
    const cards = stateCards(base, { ...GALLERY, example: true });
    expect(cards).toHaveLength(9);
    expect(JSON.stringify(cards)).toContain("epoch 9/20 · loss 0.0371");
    expect(cards.find((c) => c.id === "unsupported")?.title).toBe("Unsupported monitor");
    expect(JSON.stringify(stateCards(base, GALLERY))).not.toContain("3 min were recovered");
  });

  it("the other machine off, a single Wi-Fi, no other machine and the missing permission come out alone", () => {
    expect(ids(demoDesk(demoOptions("demo&still&trouble=peer-off")))).toContain("off");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=link-down")))).toContain("cable");
    expect(ids(demoDesk(demoOptions("demo&still&peer=none")))).toContain("first");
    expect(ids(demoDesk(demoOptions("demo&still&trouble=permissions")))).toContain("permission");
  });

  it("a pairing offer from another machine brings the card that forgets the old pair (T-345)", () => {
    const offered = (d: DeskView): DeskView => ({ ...d, pair: { peer: { kind: "paired", name: "Old Mac", link: null }, pairing: { kind: "offered" } } });
    const d = offered(demoDesk(demoOptions("demo&still&trouble=peer-off")));
    expect(ids(d)[0]).toBe("offer");
    const card = stateCards(d).find((c) => c.id === "offer");
    expect(card?.title).toBe("Old Mac is not answering");
    expect(card?.body).toContain("Another Tandem machine is looking for a pair");
    expect(card?.action).toMatchObject({ does: "forget-and-pair", label: "Forget the old pair and pair" });
    // Asking again says what is lost and asks to confirm.
    const sure = stateCards(d, { ...REAL, confirmOffer: true }).find((c) => c.id === "offer");
    expect(sure?.body).toMatch(/forgets Old Mac/);
    expect(sure?.action?.label).toBe("Confirm");
    // Never without the offer, nor in the plain nine-card gallery.
    expect(ids(demoDesk(demoOptions("demo&still&trouble=peer-off")))).not.toContain("offer");
    expect(ids(base, GALLERY)).not.toContain("offer");
  });

  it("the failed scene comes out with its name and id, not the first or the active one", () => {
    const cards = stateCards(base, { gallery: false, failedScene: { id: "game", name: "Game" } });
    const scene = cards.find((c) => c.id === "scene");
    expect(scene?.title).toBe("The scene “Game” was not applied");
    expect(scene?.action).toMatchObject({ does: "retry-scene", sceneId: "game" });
  });

  it("no text invents data: no epochs, loss, GPU or sample names", () => {
    const all = JSON.stringify(stateCards(base, GALLERY));
    for (const x of ["LoRA", "epoch", "loss", "ComfyUI", "MacBook only", "14.1", "0.6 s", "2 workspaces"]) expect(all).not.toContain(x);
  });

  it("shortPath leaves the binary path without the text around it and shortens it", () => {
    expect(shortPath("Grant the permission to this binary: /Applications/Tandem.app/Contents/MacOS/tandemd (pid 12)")).toBe("/Applications/Tandem.app/…/tandemd");
    expect(shortPath("/a/b")).toBe("/a/b");
    expect(shortPath(null)).toBe("/Applications/Tandem.app/…/tandemd");
  });
});

describe("States screen", () => {
  it("in the demo it shows the gallery with its actions", async () => {
    await mount();
    expect(screen.getByRole("heading", { level: 1, name: "Trouble states" })).toBeInTheDocument();
    expect(screen.getAllByRole("article")).toHaveLength(9);
    for (const name of ["Pair the other machine", "Power on", "View in Tunnels", "Retry", "Open System Settings"]) {
      expect(screen.getByRole("button", { name: new RegExp(name) })).toBeInTheDocument();
    }
  });

  it("no card talks about \"adding\" a machine (D1)", async () => {
    await mount();
    expect(document.body.textContent).not.toMatch(/add (another )?machine/i);
  });

  it("\"Pair the other machine\" opens the pairing setup", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openWizard");
    await userEvent.click(screen.getByRole("button", { name: /Pair the other machine/ }));
    expect(open).toHaveBeenCalledTimes(1);
  });

  it("\"Power on\" turns on the other machine", async () => {
    const store = await mount();
    const power = vi.spyOn(store, "setPower");
    await userEvent.click(screen.getByRole("button", { name: /Power on/ }));
    expect(power).toHaveBeenCalledWith("b", true);
  });

  it("\"View in Tunnels\" navigates to Tunnels", async () => {
    const store = await mount();
    await userEvent.click(screen.getByRole("button", { name: /View in Tunnels/ }));
    expect(store.route).toEqual({ screen: "tunnels" });
  });

  it("\"Open System Settings\" requests the input monitoring permission", async () => {
    const store = await mount();
    const open = vi.spyOn(store, "openPermission");
    await userEvent.click(screen.getByRole("button", { name: /Open System Settings/ }));
    expect(open).toHaveBeenCalledWith("input_monitoring");
  });

  it("\"Retry\" reapplies the scene that failed, not the first", async () => {
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

describe("the scene that failed is remembered in the store", () => {
  it("when a scene fails it is noted and cleared when another is applied well", async () => {
    const store = new Store(new DemoSource(demoOptions("demo&still")));
    await store.start();
    expect(store.demo).toBe(true);
    expect(store.failedScene).toBeNull();
  });
});
