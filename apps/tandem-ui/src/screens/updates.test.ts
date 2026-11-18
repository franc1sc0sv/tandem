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
  await userEvent.click(screen.getByRole("tab", { name: "Updates" }));
  await screen.findByText("Tandem 0.3.0");
  return store;
}

describe("Ajustes → Actualizaciones (F-Settings, T-122)", () => {
  it("muestra la versión de las dos máquinas, el origen y «Update only» encendido", async () => {
    await openUpdates();
    expect(screen.getByText("All machines of the desk on the same version.")).toBeInTheDocument();
    expect(screen.getByText("GitHub Releases · unsigned")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Update only" })).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByRole("button", { name: /Update all/ })).toBeNull();
  });

  it("buscar ahora encuentra la versión nueva y «Update all» empieza por the other machine", async () => {
    await openUpdates();
    await userEvent.click(screen.getByRole("button", { name: /Check now/ }));
    expect(await screen.findByText("A new version is available: 0.3.1")).toBeInTheDocument();
    expect(screen.getByText("It installs on every machine, one at a time, without cutting the keyboard.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Check again/ })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Update all/ }));
    expect(await screen.findByText("Asking the other machine to update…")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Update all/ })).toBeDisabled();
  });

  it("el interruptor se guarda", async () => {
    const store = await openUpdates();
    const sw = screen.getByRole("switch", { name: "Update only" });
    await userEvent.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "false");
    expect((await store.updates(() => {})).auto).toBe(false);
  });
});
