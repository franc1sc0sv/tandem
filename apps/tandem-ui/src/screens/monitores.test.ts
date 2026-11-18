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

describe("Monitores (V3-Monitores) en modo demo", () => {
  it("el 34\" compatible trae su selector, su dueño y sus eventos", async () => {
    await open("#/monitores");
    const card = screen.getByRole("article", { name: 'Odyssey G5 34"' });
    expect(within(card).getByText("Estándar Tandem")).toBeInTheDocument();
    const radios = within(picker()).getAllByRole("radio");
    expect(radios).toHaveLength(2);
    expect(radios[0]).toHaveTextContent("MacBook Pro");
    expect(radios[1]).toHaveTextContent("PC-Escritorio");
    expect(within(card).getAllByText(/Entrada →|Cambio|Señal/).length).toBeGreaterThan(0);
  });

  it("D11: el 32\" se muestra «no compatible», con su razón, sin selector ni cambio de entrada", async () => {
    await open("#/monitores");
    const card = screen.getByRole("article", { name: /Odyssey G5 32", no compatible/ });
    expect(within(card).getByText("No compatible")).toBeInTheDocument();
    expect(within(card).getByText(/firmware cumpla el estándar/)).toBeInTheDocument();
    expect(within(card).queryByRole("radiogroup")).toBeNull();
    expect(screen.getAllByRole("radiogroup")).toHaveLength(1);
  });

  it("«Qué es el estándar» abre y cierra la explicación", async () => {
    await open("#/monitores");
    const b = screen.getByRole("button", { name: "Qué es el estándar" });
    expect(b).toHaveAttribute("aria-pressed", "false");
    expect(screen.queryByText(/habla el protocolo del firmware propio/)).toBeNull();
    await userEvent.click(b);
    expect(b).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/habla el protocolo del firmware propio/)).toBeInTheDocument();
    await userEvent.click(b);
    expect(screen.queryByText(/habla el protocolo del firmware propio/)).toBeNull();
  });

  it("elegir la otra máquina pide el cambio de entrada y el selector sigue lo que muestra el monitor", async () => {
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
    const active = cards.filter((c) => within(c).queryByText("Activa"));
    expect(active).toHaveLength(1);
    expect(within(active[0] as HTMLElement).queryByRole("button", { name: "Aplicar" })).toBeNull();
    for (const c of cards.filter((x) => x !== active[0])) expect(within(c).getByRole("button", { name: "Aplicar" })).toBeEnabled();
  });

  it("«Aplicar» lleva al progreso paso a paso y se puede volver a Monitores", async () => {
    const store = await open("#/monitores");
    const other = screen.getAllByRole("article", { name: /^Escena / }).find((c) => !within(c).queryByText("Activa")) as HTMLElement;
    await userEvent.click(within(other).getByRole("button", { name: "Aplicar" }));
    await waitFor(() => expect(store.route.screen).toBe("progreso"));
    expect(await screen.findByRole("heading", { name: "Cambios en progreso" })).toBeInTheDocument();
    expect(screen.getByText("Guardar cómo está todo")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Volver a Monitores" })).toHaveAttribute("href", "#/monitores");
  });
});

describe("Nueva escena (F-NuevaEscena) en modo demo", () => {
  it("no se guarda sin nombre; con nombre y la otra máquina queda como tarjeta en Monitores", async () => {
    const store = await open("#/monitores/escena");
    const save = screen.getByRole("button", { name: "Guardar" });
    expect(save).toBeDisabled();
    await userEvent.type(screen.getByRole("textbox"), "Juego 2");
    expect(save).toBeEnabled();
    await userEvent.click(screen.getByRole("radio", { name: /PC-Escritorio/ }));
    expect(screen.getByText("PC-Escritorio", { selector: "b" })).toBeInTheDocument();
    await userEvent.click(save);
    await waitFor(() => expect(store.route.screen).toBe("monitores"));
    expect(await screen.findByRole("article", { name: "Escena Juego 2" })).toBeInTheDocument();
  });

  it("el atajo se graba con la combinación y avisa si ya es de otra escena", async () => {
    await open("#/monitores/escena");
    await userEvent.type(screen.getByRole("textbox"), "Otra");
    const kbd = screen.getByRole("button", { name: "Atajo" });
    expect(kbd).toHaveTextContent("Sin atajo");
    kbd.focus();
    await userEvent.keyboard("{Control>}{Alt>}4{/Alt}{/Control}");
    expect(kbd.textContent).not.toBe("Sin atajo");
    await userEvent.keyboard("{Backspace}");
    expect(kbd).toHaveTextContent("Sin atajo");
  });

  it("«Guardar y aplicar» guarda y abre el progreso", async () => {
    const store = await open("#/monitores/escena");
    await userEvent.type(screen.getByRole("textbox"), "Con progreso");
    await userEvent.click(screen.getByRole("radio", { name: /PC-Escritorio/ }));
    await userEvent.click(screen.getByRole("button", { name: "Guardar y aplicar" }));
    await waitFor(() => expect(store.route.screen).toBe("progreso"));
  });
});

describe("Progreso (F-Transacciones) en modo demo", () => {
  it("una escena en curso muestra sus pasos y el aviso de que el monitor confirma cada cambio", async () => {
    await open("#/progreso", "demo&still&txn=run");
    expect(await screen.findByText("Aplicando la escena «Juego»")).toBeInTheDocument();
    expect(screen.getByText("El monitor confirma cada cambio; no se adivina.")).toBeInTheDocument();
    expect(screen.getByText("esperando al monitor")).toBeInTheDocument();
  });

  it("al terminar bien dice que la escena quedó activa", async () => {
    await open("#/progreso", "demo&still&txn=ok");
    expect(await screen.findByText(/Escena «Juego» activa/)).toBeInTheDocument();
  });

  it("si el monitor no da imagen se ve la vuelta atrás y nada queda a medias", async () => {
    await open("#/progreso", "demo&still&txn=fail");
    expect(await screen.findByText("Volviendo a como estaba")).toBeInTheDocument();
    expect(screen.getByText("el monitor dijo: cambio fallido")).toBeInTheDocument();
    expect(screen.getByText(/nada quedó a medias/)).toBeInTheDocument();
  });
});
