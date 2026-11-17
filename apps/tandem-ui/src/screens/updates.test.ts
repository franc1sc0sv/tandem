import { render, screen } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { describe, expect, it } from "vitest";

import { STORE_KEY } from "../lib/context";
import { DemoSource, demoOptions } from "../lib/store/demo";
import { Store } from "../lib/store/store.svelte";
import Shell from "../shell/Shell.svelte";

async function openUpdates() {
  const store = new Store(new DemoSource(demoOptions("demo&still")), { hash: "#/ajustes" });
  await store.start();
  render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  await userEvent.click(screen.getByRole("tab", { name: "Actualizaciones" }));
  await screen.findByText("Tandem 0.3.0");
  return store;
}

describe("Ajustes → Actualizaciones (F-Ajustes, T-122)", () => {
  it("muestra la versión de las dos máquinas, el origen y «Actualizar solo» encendido", async () => {
    await openUpdates();
    expect(screen.getByText("Todas las máquinas del puesto en la misma versión.")).toBeInTheDocument();
    expect(screen.getByText("GitHub Releases · sin firma")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Actualizar solo" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByRole("button", { name: /Actualizar todas/ })).toBeNull();
  });

  it("buscar ahora encuentra la versión nueva y «Actualizar todas» empieza por la otra máquina", async () => {
    await openUpdates();
    await userEvent.click(screen.getByRole("button", { name: /Buscar ahora/ }));
    expect(await screen.findByText("Hay una versión nueva: 0.3.1")).toBeInTheDocument();
    expect(screen.getByText("Se instala en todas las máquinas, una a la vez, sin cortar el teclado.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Buscar de nuevo/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Actualizar todas/ }));
    expect(await screen.findByText("Pidiendo a la otra máquina que se actualice…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Actualizar todas/ })).toBeDisabled();
  });

  it("el interruptor se guarda", async () => {
    const store = await openUpdates();
    const sw = screen.getByRole("switch", { name: "Actualizar solo" });
    await userEvent.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect((await store.updates(() => {})).auto).toBe(false);
  });
});
