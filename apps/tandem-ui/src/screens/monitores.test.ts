import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";

async function open(hash: string, query = "demo&still") {
  const store = new Store(new DemoSource(demoOptions(query)), { hash });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return store;
}

const picker = () => screen.getByRole("radiogroup", { name: "Qué máquina muestra el 34 pulgadas" });

describe("Monitores (V3-Monitors) en modo demo", () => {
  it("el 34\" compatible trae su selector, su dueño y sus eventos", async () => {
    await open("#/monitores");
    const card = screen.getByRole("article", { name: 'Odyssey G5 34"' });
    expect(within(card).getByText("Tandem standard")).toBeInTheDocument();
    const radios = within(picker()).getAllByRole("radio");
    expect(radios).toHaveLength(2);
    expect(radios[0]).toHaveTextContent("MacBook Pro");
    expect(radios[1]).toHaveTextContent("PC-Desktop");
    expect(within(card).getAllByText(/Entrada →|Cambio|Señal/).length).toBeGreaterThan(0);
  });

  it("D11: el 32\" se muestra «no compatible», con su razón, sin selector ni cambio de entrada", async () => {
    await open("#/monitores");
    const card = screen.getByRole("article", { name: /Odyssey G5 32", no compatible/ });
    expect(within(card).getByText("Unsupported")).toBeInTheDocument();
    expect(within(card).getByText(/firmware cumpla el estándar/)).toBeInTheDocument();
    expect(within(card).queryByRole("radiogroup")).toBeNull();
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
  });

  it("«What the standard is» abre y cierra la explicación", async () => {
    await open("#/monitores");
    const b = screen.getByRole("button", { name: "What the standard is" });
    expect(b).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByText(/habla el protocolo del firmware propio/)).toBeNull();
    await userEvent.click(b);
    expect(b).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/habla el protocolo del firmware propio/)).toBeInTheDocument();
    await userEvent.click(b);
    expect(screen.queryByText(/habla el protocolo del firmware propio/)).toBeNull();
  });

  it("elegir the other machine pide el cambio de entrada y el selector sigue lo que muestra the monitor", async () => {
    await open("#/monitores");
    const radios = () => within(picker()).getAllByRole("radio");
    expect(radios()[1]).toHaveAttribute("aria-checked", "true");
    await userEvent.click(radios()[0] as HTMLElement);
    await waitFor(() => expect(radios()[0]).toHaveAttribute("aria-checked", "true"));
    expect(radios()[1]).toHaveAttribute("aria-checked", "false");
  });

  it("las escenas son tarjetas: la activa lleva «Activa» y las demás «Aplicar»", async () => {
    await open("#/monitores");
    const cards = screen.getAllByRole("article", { name: /^Escena / });
    expect(cards.length).toBeGreaterThan(1);
    const active = cards.filter((c) => within(c).queryByText("Active"));
    expect(active).toHaveLength(1);
    expect(within(active[0] as HTMLElement).queryByRole("button", { name: "Apply" })).toBeNull();
    for (const c of cards.filter((x) => x !== active[0])) expect(within(c).getByRole("button", { name: "Apply" })).toBeEnabled();
  });

  it("«Aplicar» lleva al progreso paso a paso y se puede volver a Monitores", async () => {
    const store = await open("#/monitores");
    const other = screen.getAllByRole("article", { name: /^Escena / }).find((c) => !within(c).queryByText("Active")) as HTMLElement;
    await userEvent.click(within(other).getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(store.route.screen).toBe("progreso"));
    expect(await screen.findByRole("heading", { name: "Changes in progress" })).toBeInTheDocument();
    expect(screen.getByText("Save how everything is")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to Monitors" })).toHaveAttribute("href", "#/monitores");
  });
});

describe("New scene (F-NewScene) en modo demo", () => {
  it("no se guarda sin nombre; con nombre y the other machine queda como tarjeta en Monitores", async () => {
    const store = await open("#/monitores/escena");
    const save = screen.getByRole("button", { name: "Save" });
    expect(save).toBeDisabled();
    await userEvent.type(screen.getByRole("textbox"), "Juego 2");
    expect(save).toBeEnabled();
    await userEvent.click(screen.getByRole("radio", { name: /PC-Escritorio/ }));
    expect(screen.getByText("PC-Desktop", { selector: "b" })).toBeInTheDocument();
    await userEvent.click(save);
    await waitFor(() => expect(store.route.screen).toBe("monitores"));
    expect(await screen.findByRole("article", { name: "Escena Juego 2" })).toBeInTheDocument();
  });

  it("el atajo se graba con la combinación y avisa si ya es de otra escena", async () => {
    await open("#/monitores/escena");
    await userEvent.type(screen.getByRole("textbox"), "Otra");
    const kbd = screen.getByRole("button", { name: "Shortcut" });
    expect(kbd).toHaveTextContent("No shortcut");
    kbd.focus();
    await userEvent.keyboard("{Control>}{Alt>}4{/Alt}{/Control}");
    expect(kbd.textContent).not.toBe("No shortcut");
    await userEvent.keyboard("{Backspace}");
    expect(kbd).toHaveTextContent("No shortcut");
  });

  it("«Save and apply» guarda y abre el progreso", async () => {
    const store = await open("#/monitores/escena");
    await userEvent.type(screen.getByRole("textbox"), "Con progreso");
    await userEvent.click(screen.getByRole("radio", { name: /PC-Escritorio/ }));
    await userEvent.click(screen.getByRole("button", { name: "Save and apply" }));
    await waitFor(() => expect(store.route.screen).toBe("progreso"));
  });
});

describe("Progreso (F-Transactions) en modo demo", () => {
  it("una escena in progress muestra sus pasos y el aviso de que the monitor confirma cada cambio", async () => {
    await open("#/progreso", "demo&still&txn=run");
    expect(await screen.findByText("Applying the scene “Game”")).toBeInTheDocument();
    expect(screen.getByText("The monitor confirms each change; nothing is guessed.")).toBeInTheDocument();
    expect(screen.getByText("waiting for the monitor")).toBeInTheDocument();
  });

  it("al terminar bien dice que la escena quedó activa", async () => {
    await open("#/progreso", "demo&still&txn=ok");
    expect(await screen.findByText(/Escena «Juego» activa/)).toBeInTheDocument();
  });

  it("si the monitor no da imagen se ve la vuelta atrás y nada queda a medias", async () => {
    await open("#/progreso", "demo&still&txn=fail");
    expect(await screen.findByText("Rolling back to how it was")).toBeInTheDocument();
    expect(screen.getByText("the monitor said: change failed")).toBeInTheDocument();
    expect(screen.getByText(/nada quedó a medias/)).toBeInTheDocument();
  });
});

describe("Changes in progress (F-Transactions): una tarjeta por transacción", () => {
  it("con txn=run salen la escena, el encendido y la medición del enlace", async () => {
    await open("#/progreso", "demo&still&txn=run");
    expect(screen.getByRole("region", { name: "Applying the scene “Game”" })).toBeInTheDocument();
    const energy = screen.getByRole("region", { name: "Encendiendo PC-Escritorio" });
    expect(within(energy).getByText("0:18 / 2:00")).toBeInTheDocument();
    expect(within(energy).queryByRole("button")).toBeNull();
    const link = screen.getByRole("region", { name: "Midiendo el enlace USB-C" });
    expect(within(link).getByText("9.4")).toBeInTheDocument();
    expect(within(link).getByRole("img", { name: "9.4 de 10 Gbps" })).toBeInTheDocument();
    expect(within(link).getByText("410 Mbps · 3.8 ms")).toBeInTheDocument();
  });
});
