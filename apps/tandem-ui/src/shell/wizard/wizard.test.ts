import { fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../../lib/context";
import { DemoSource, demoOptions } from "../../lib/store/demo";
import { Store } from "../../lib/store/store.svelte";
import { MCP_COMMAND } from "../../lib/wizard";
import Shell from "../Shell.svelte";

async function mount(query = "demo&still", step = 0, hash = "#/desk") {
  const source = new DemoSource(demoOptions(query));
  const store = new Store(source, { hash });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store, source, dlg: screen.getByRole("dialog", { name: "Setup" }) };
}

afterEach(() => vi.useRealTimers());

describe("lista de pasos", () => {
  it("tiene los seis pasos, clicables, con hechos, actual y por hacer", async () => {
    const { dlg, store } = await mount("demo&still", 2);
    const steps = within(within(dlg).getByRole("complementary", { name: "Steps" })).getAllByRole("button").filter((b) => /^[\d✓]/.test(b.textContent ?? ""));
    expect(steps.map((b) => b.textContent)).toEqual(["✓This machine", "✓Your desk", "3Permissions", "4Keyboard and mouse", "5Arrangement and crossing test", "6Done"]);
    expect(steps[0]).toHaveClass("done");
    expect(steps[2]).toHaveAttribute("aria-current", "step");
    expect(steps[3]).not.toHaveClass("done");
    await userEvent.click(steps[4]!);
    expect(store.wizardStep).toBe(4);
    expect(within(dlg).getByRole("heading", { name: "Where is each screen?" })).toBeInTheDocument();
  });

  it("Continuar y Atrás recorren los pasos; Escape lo cierra", async () => {
    const { dlg, store } = await mount();
    await userEvent.click(within(dlg).getByRole("button", { name: "Continue" }));
    expect(store.wizardStep).toBe(1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Back" }));
    expect(store.wizardStep).toBe(0);
    await fireEvent.keyDown(dlg, { key: "Escape" });
    expect(store.wizardOpen).toBe(false);
  });
});

describe("paso 1: this machine", () => {
  it("lista lo que encontró, con los textos del escaneo", async () => {
    const { dlg } = await mount();
    const found = within(dlg).getByRole("region", { name: "What it found" });
    for (const t of ["macOS 26 · Apple M5 · 24 GB", "capture and injection", "text, HTML, image", "zsh · Docker · uv · node", "no DDC channel right now; another machine controls them", "Administrator service"]) {
      expect(within(found).getByText(new RegExp(t))).toBeInTheDocument();
    }
    expect(within(found).queryByRole("button", { name: "Install" })).toBeNull();
  });

  it("con el servicio pendiente muestra «Instalar»; al instalar la fila queda lista y el botón desaparece", async () => {
    const { dlg } = await mount("demo&still&admin=pending");
    const found = within(dlg).getByRole("region", { name: "What it found" });
    const btn = await within(found).findByRole("button", { name: "Install" });
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(1);
    await userEvent.click(btn);
    await waitFor(() => expect(within(found).queryByRole("button", { name: "Install" })).toBeNull());
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(0);
  });

  it("sin instalador (per-user install) la fila lo explica y no ofrece el botón", async () => {
    const { dlg } = await mount("demo&still&admin=off");
    const found = within(dlg).getByRole("region", { name: "What it found" });
    expect(await within(found).findByText(/per-user install/)).toBeInTheDocument();
    expect(within(found).queryByRole("button", { name: "Install" })).toBeNull();
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(0);
  });

  it("el nombre y el color se guardan en el desk", async () => {
    const { dlg, store } = await mount();
    const input = within(dlg).getByRole("textbox", { name: "Name" });
    await userEvent.clear(input);
    await userEvent.type(input, "Laptop{Enter}");
    expect(store.desk.machines.a?.name).toBe("Laptop");
    await userEvent.click(within(dlg).getByRole("radio", { name: "Amber" }));
    expect(store.desk.machines.a?.color).toBe("#d9a75a");
    expect(within(dlg).getByRole("radio", { name: "Amber" })).toHaveAttribute("aria-checked", "true");
    // Un nombre vacío no se guarda.
    await userEvent.clear(input);
    await userEvent.tab();
    expect(store.desk.machines.a?.name).toBe("Laptop");
  });
});

describe("paso 2: tu desk", () => {
  it("unirse: busca sola, el usuario escribe el código y las dos quedan emparejadas", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg, store } = await mount("demo&still&peer=none", 1);
    expect(await within(dlg).findByText("Looking for PC-Desktop on your network…")).toBeInTheDocument();
    await vi.advanceTimersByTimeAsync(1500);
    expect(within(dlg).getByText("Found on your network")).toBeInTheDocument();
    expect(within(dlg).getByText("Enter the code PC-Desktop shows")).toBeInTheDocument();
    // Un código equivocado no confirma.
    for (const [i, d] of [..."418028"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Digit ${i + 1}` }), d);
    expect(within(dlg).getByRole("alert")).toHaveTextContent("That is not the code PC-Desktop shows");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: false });
    // El correcto sí.
    await userEvent.clear(within(dlg).getByRole("textbox", { name: "Digit 6" }));
    await userEvent.type(within(dlg).getByRole("textbox", { name: "Digit 6" }), "7");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: true });
    await vi.advanceTimersByTimeAsync(1300);
    expect(store.desk.pair.peer).toMatchObject({ kind: "paired", name: "PC-Desktop" });
    expect(await within(dlg).findByText(/Paired/)).toBeInTheDocument();
  });

  it("crear: muestra el código y confirma sin pedir nada más", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg, store } = await mount("demo&still&peer=none", 1);
    await userEvent.click(within(dlg).getByRole("radio", { name: /Create a new desk/ }));
    await vi.advanceTimersByTimeAsync(1500);
    expect(within(dlg).getByRole("group", { name: /Code of MacBook Pro/ }).textContent).toBe("418027");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: true });
    expect(within(dlg).getByText(/Then, on the other machine/)).toBeInTheDocument();
  });

  it("emparejadas: «Change the other machine» pide confirmar antes de desemparejar", async () => {
    const { dlg, store } = await mount("demo&still", 1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Change the other machine" }));
    expect(within(dlg).getByText("Unpair PC-Desktop?")).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "No" }));
    expect(store.desk.pair.peer.kind).toBe("paired");
    await userEvent.click(within(dlg).getByRole("button", { name: "Change the other machine" }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Unpair" }));
    expect(store.desk.pair.peer.kind).toBe("unpaired");
  });

  it("un rechazo en the other machine se cuenta y se puede reintentar", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg } = await mount("demo&still&peer=none&pair=reject", 1);
    await vi.advanceTimersByTimeAsync(1500);
    for (const [i, d] of [..."418027"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Digit ${i + 1}` }), d);
    await vi.advanceTimersByTimeAsync(1300);
    expect(await within(dlg).findByText("They did not pair")).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Retry" })).toBeInTheDocument();
  });
});

describe("paso 3: permisos", () => {
  it("los pide, muestra los que faltan y pasan a concedidos al abrirlos", async () => {
    const { dlg, store } = await mount("demo&still&perms=missing", 2);
    expect(within(dlg).getByRole("heading", { name: "macOS permissions" })).toBeInTheDocument();
    expect(within(dlg).getByText("Missing 2")).toBeInTheDocument();
    const next = within(dlg).getByRole("button", { name: "Continue" });
    expect(next).toHaveClass("wait");
    expect(within(dlg).getAllByText("Granted · checked just now")).toHaveLength(1);
    const open = within(dlg).getAllByRole("button", { name: "Open settings" });
    expect(open).toHaveLength(2);
    await userEvent.click(open[0]!);
    expect(within(dlg).getByText("Missing 1")).toBeInTheDocument();
    // Tras abrir un panel se ofrece reiniciar el agente.
    expect(within(dlg).getByRole("button", { name: "Restart agent" })).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Open settings" }));
    expect(store.desk.permissions.rows.filter((r) => r.state === "missing")).toEqual([]);
    expect(within(dlg).getByText("All granted")).toBeInTheDocument();
    expect(next).not.toHaveClass("wait");
    expect(within(dlg).queryByRole("button", { name: "Restart agent" })).toBeNull();
  });

  it("en Windows no hace falta ningún permiso", async () => {
    const { dlg } = await mount("demo&still&here=b", 2);
    expect(within(dlg).getByText("This machine needs no permissions")).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Continue" })).not.toHaveClass("wait");
  });
});

describe("paso 4: teclado y mouse", () => {
  it("cada machine lista lo que tiene; la que no tiene nada lo dice", async () => {
    const { dlg } = await mount("demo&still", 3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(mac).getByText("Built-in keyboard")).toBeInTheDocument();
    expect(within(mac).getByText("Trackpad")).toBeInTheDocument();
    expect(within(mac).getByText(/Keychron K3 · MX Master 3S/)).toBeInTheDocument();
    const pc = within(dlg).getByRole("group", { name: "PC-Desktop" });
    expect(within(pc).getByText("No keyboard or mouse right now")).toBeInTheDocument();
    expect(within(pc).getByText("If you plug the hub in here, it detects it on its own.")).toBeInTheDocument();
  });

  it("marca la actividad de cada machine cuando la oye (agente y ventana)", async () => {
    const { dlg, source } = await mount("demo&still", 3);
    const pc = within(dlg).getByRole("group", { name: "PC-Desktop" });
    expect(within(pc).queryByText("Activity detected")).toBeNull();
    source.heardKey("b");
    expect(await within(pc).findByText("Activity detected")).toBeInTheDocument();
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(mac).queryByText("Activity detected")).toBeNull();
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Activity detected")).toBeInTheDocument();
  });
});

describe("paso 4: el puntero quieto no es actividad", () => {
  it("un `pointermove` sin desplazamiento (el navegador lo emite al cambiar el diseño) no marca nada", async () => {
    const { dlg } = await mount("demo&still", 3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    const still = new Event("pointermove");
    Object.assign(still, { movementX: 0, movementY: 0 });
    window.dispatchEvent(still);
    await tick();
    expect(within(mac).queryByText("Activity detected")).toBeNull();
    const moved = new Event("pointermove");
    Object.assign(moved, { movementX: 3, movementY: 0 });
    window.dispatchEvent(moved);
    expect(await within(mac).findByText("Activity detected")).toBeInTheDocument();
  });
});

describe("paso 5: arrangement y prueba de crossing", () => {
  it("espera el crossing y, si pasa, lo dice y habilita Continuar", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    const next = within(dlg).getByRole("button", { name: "Continue" });
    expect(next).toHaveClass("wait");
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    expect(await within(dlg).findByText(/Crossed to PC-Desktop through the right edge, as the arrangement says. Round trip measured: 0.4 ms./)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(true);
    expect(next).not.toHaveClass("wait");
  });

  it("por otro borde falla y se puede probar de fresh", async () => {
    const { dlg, store } = await mount("demo&still&cross=wrong", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    expect(await within(dlg).findByText(/Crossed at the top, not the right/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
    expect(within(dlg).getByRole("button", { name: "Test again" })).toBeInTheDocument();
  });

  it("sin cruzar a tiempo, falla con el borde que había que usar", async () => {
    const { dlg } = await mount("demo&still&cross=timeout", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    expect(await within(dlg).findByText(/Did not cross in 20 s/)).toBeInTheDocument();
  });

  it("mover the other machine con las flechas guarda el arrangement y reinicia la prueba", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Test crossing" }));
    await within(dlg).findByText(/Crossed to/);
    const other = within(dlg).getByRole("button", { name: /Move PC-Desktop/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    // A (aquí) sale the bottom: B queda debajo.
    expect(store.desk.layout.arrangement.aSide).toBe("up");
    expect(store.wizardCrossed).toBe(false);
    expect(within(dlg).getByRole("button", { name: "Test crossing" })).toBeInTheDocument();
  });

  it("Saltar pasa al resumen sin la prueba", async () => {
    const { dlg, store } = await mount("demo&still", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Skip" }));
    expect(store.wizardStep).toBe(5);
    expect(within(dlg).getByText("crossing not tested")).toBeInTheDocument();
  });
});

describe("paso 6: listo", () => {
  it("resume lo comprobado y copia el comando de MCP", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { dlg, store } = await mount("demo&still", 5);
    store.wizardCrossed = true;
    await tick();
    const sum = within(dlg).getByRole("region", { name: "Summary" });
    expect(sum).toHaveTextContent("MacBook Pro and PC-Desktop in “My desk”");
    expect(sum).toHaveTextContent("USB-C · 0.3 ms");
    expect(sum).toHaveTextContent("from any machine · crossing tested");
    expect(sum).toHaveTextContent("MCP ready for Claude Code on both");
    expect(within(dlg).getByText(MCP_COMMAND)).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Copy command" }));
    expect(writeText).toHaveBeenCalledWith("claude mcp add tandem -- tandem mcp");
  });

  it("«Go to desk» cierra el asistente", async () => {
    const { dlg, store } = await mount("demo&still", 5);
    await userEvent.click(within(dlg).getByRole("button", { name: "Go to desk" }));
    expect(store.wizardOpen).toBe(false);
    expect(store.route).toEqual({ screen: "desk" });
  });
});
