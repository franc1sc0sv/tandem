import { fireEvent, render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick } from "svelte";
import { afterEach, describe, expect, it, vi } from "vitest";

import { STORE_KEY } from "../../lib/context";
import { DemoSource, demoOptions } from "../../lib/store/demo";
import { Store } from "../../lib/store/store.svelte";
import { MCP_COMMAND } from "../../lib/wizard";
import Shell from "../Shell.svelte";

async function mount(query = "demo&still", step = 0, hash = "#/puesto") {
  const source = new DemoSource(demoOptions(query));
  const store = new Store(source, { hash });
  await store.start();
  store.openWizard(step);
  const r = render(Shell, { context: new Map([[STORE_KEY, store]]) });
  await tick();
  return { ...r, store, source, dlg: screen.getByRole("dialog", { name: "Asistente" }) };
}

afterEach(() => vi.useRealTimers());

describe("lista de pasos", () => {
  it("tiene los seis pasos, clicables, con hechos, actual y por hacer", async () => {
    const { dlg, store } = await mount("demo&still", 2);
    const steps = within(within(dlg).getByRole("complementary", { name: "Pasos" })).getAllByRole("button").filter((b) => /^[\d✓]/.test(b.textContent ?? ""));
    expect(steps.map((b) => b.textContent)).toEqual(["✓Esta máquina", "✓Tu puesto", "3Permisos", "4Teclado y mouse", "5Acomodo y prueba de cruce", "6Listo"]);
    expect(steps[0]).toHaveClass("done");
    expect(steps[2]).toHaveAttribute("aria-current", "step");
    expect(steps[3]).not.toHaveClass("done");
    await userEvent.click(steps[4]!);
    expect(store.wizardStep).toBe(4);
    expect(within(dlg).getByRole("heading", { name: "¿Dónde está cada pantalla?" })).toBeInTheDocument();
  });

  it("Continuar y Atrás recorren los pasos; Escape lo cierra", async () => {
    const { dlg, store } = await mount();
    await userEvent.click(within(dlg).getByRole("button", { name: "Continuar" }));
    expect(store.wizardStep).toBe(1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Atrás" }));
    expect(store.wizardStep).toBe(0);
    await fireEvent.keyDown(dlg, { key: "Escape" });
    expect(store.wizardOpen).toBe(false);
  });
});

describe("paso 1: esta máquina", () => {
  it("lista lo que encontró, con los textos del escaneo", async () => {
    const { dlg } = await mount();
    const found = within(dlg).getByRole("region", { name: "Lo que encontró" });
    for (const t of ["macOS 26 · Apple M5 · 24 GB", "captura e inyección", "texto, HTML, imagen", "zsh · Docker · uv · node", "sin canal DDC ahora; otra máquina los controla", "Servicio de administrador"]) {
      expect(within(found).getByText(new RegExp(t))).toBeInTheDocument();
    }
    expect(within(found).queryByRole("button", { name: "Instalar" })).toBeNull();
  });

  it("con el servicio pendiente muestra «Instalar»; al instalar la fila queda lista y el botón desaparece", async () => {
    const { dlg } = await mount("demo&still&admin=pending");
    const found = within(dlg).getByRole("region", { name: "Lo que encontró" });
    const btn = await within(found).findByRole("button", { name: "Instalar" });
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(1);
    await userEvent.click(btn);
    await waitFor(() => expect(within(found).queryByRole("button", { name: "Instalar" })).toBeNull());
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(0);
  });

  it("sin instalador (instalación por usuario) la fila lo explica y no ofrece el botón", async () => {
    const { dlg } = await mount("demo&still&admin=off");
    const found = within(dlg).getByRole("region", { name: "Lo que encontró" });
    expect(await within(found).findByText(/instalación por usuario/)).toBeInTheDocument();
    expect(within(found).queryByRole("button", { name: "Instalar" })).toBeNull();
    expect(found.querySelectorAll(".wz-spin")).toHaveLength(0);
  });

  it("el nombre y el color se guardan en el puesto", async () => {
    const { dlg, store } = await mount();
    const input = within(dlg).getByRole("textbox", { name: "Nombre" });
    await userEvent.clear(input);
    await userEvent.type(input, "Portátil{Enter}");
    expect(store.desk.machines.a?.name).toBe("Portátil");
    await userEvent.click(within(dlg).getByRole("radio", { name: "Ámbar" }));
    expect(store.desk.machines.a?.color).toBe("#d9a75a");
    expect(within(dlg).getByRole("radio", { name: "Ámbar" })).toHaveAttribute("aria-checked", "true");
    // Un nombre vacío no se guarda.
    await userEvent.clear(input);
    await userEvent.tab();
    expect(store.desk.machines.a?.name).toBe("Portátil");
  });
});

describe("paso 2: tu puesto", () => {
  it("unirse: busca sola, el usuario escribe el código y las dos quedan emparejadas", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg, store } = await mount("demo&still&peer=none", 1);
    expect(await within(dlg).findByText("Buscando a PC-Escritorio en tu red…")).toBeInTheDocument();
    await vi.advanceTimersByTimeAsync(1500);
    expect(within(dlg).getByText("Encontrada en tu red")).toBeInTheDocument();
    expect(within(dlg).getByText("Escribe el código que muestra PC-Escritorio")).toBeInTheDocument();
    // Un código equivocado no confirma.
    for (const [i, d] of [..."418028"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Dígito ${i + 1}` }), d);
    expect(within(dlg).getByRole("alert")).toHaveTextContent("No es el código que muestra PC-Escritorio");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: false });
    // El correcto sí.
    await userEvent.clear(within(dlg).getByRole("textbox", { name: "Dígito 6" }));
    await userEvent.type(within(dlg).getByRole("textbox", { name: "Dígito 6" }), "7");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: true });
    await vi.advanceTimersByTimeAsync(1300);
    expect(store.desk.pair.peer).toMatchObject({ kind: "paired", name: "PC-Escritorio" });
    expect(await within(dlg).findByText(/Emparejada/)).toBeInTheDocument();
  });

  it("crear: muestra el código y confirma sin pedir nada más", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg, store } = await mount("demo&still&peer=none", 1);
    await userEvent.click(within(dlg).getByRole("radio", { name: /Crear un puesto nuevo/ }));
    await vi.advanceTimersByTimeAsync(1500);
    expect(within(dlg).getByRole("group", { name: /Código de MacBook Pro/ }).textContent).toBe("418027");
    expect(store.desk.pair.pairing).toMatchObject({ kind: "comparing", confirmed: true });
    expect(within(dlg).getByText(/Después, en la otra máquina/)).toBeInTheDocument();
  });

  it("emparejadas: «Cambiar la otra máquina» pide confirmar antes de desemparejar", async () => {
    const { dlg, store } = await mount("demo&still", 1);
    await userEvent.click(within(dlg).getByRole("button", { name: "Cambiar la otra máquina" }));
    expect(within(dlg).getByText("¿Desemparejar PC-Escritorio?")).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "No" }));
    expect(store.desk.pair.peer.kind).toBe("paired");
    await userEvent.click(within(dlg).getByRole("button", { name: "Cambiar la otra máquina" }));
    await userEvent.click(within(dlg).getByRole("button", { name: "Desemparejar" }));
    expect(store.desk.pair.peer.kind).toBe("unpaired");
  });

  it("un rechazo en la otra máquina se cuenta y se puede reintentar", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const { dlg } = await mount("demo&still&peer=none&pair=reject", 1);
    await vi.advanceTimersByTimeAsync(1500);
    for (const [i, d] of [..."418027"].entries()) await userEvent.type(within(dlg).getByRole("textbox", { name: `Dígito ${i + 1}` }), d);
    await vi.advanceTimersByTimeAsync(1300);
    expect(await within(dlg).findByText("No se emparejaron")).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Reintentar" })).toBeInTheDocument();
  });
});

describe("paso 3: permisos", () => {
  it("los pide, muestra los que faltan y pasan a concedidos al abrirlos", async () => {
    const { dlg, store } = await mount("demo&still&perms=missing", 2);
    expect(within(dlg).getByRole("heading", { name: "Permisos de macOS" })).toBeInTheDocument();
    expect(within(dlg).getByText("Faltan 2")).toBeInTheDocument();
    const next = within(dlg).getByRole("button", { name: "Continuar" });
    expect(next).toHaveClass("wait");
    expect(within(dlg).getAllByText("Concedido · comprobado ahora")).toHaveLength(1);
    const open = within(dlg).getAllByRole("button", { name: "Abrir ajustes" });
    expect(open).toHaveLength(2);
    await userEvent.click(open[0]!);
    expect(within(dlg).getByText("Falta 1")).toBeInTheDocument();
    // Tras abrir un panel se ofrece reiniciar el agente.
    expect(within(dlg).getByRole("button", { name: "Reiniciar agente" })).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Abrir ajustes" }));
    expect(store.desk.permissions.rows.filter((r) => r.state === "missing")).toEqual([]);
    expect(within(dlg).getByText("Todo concedido")).toBeInTheDocument();
    expect(next).not.toHaveClass("wait");
    expect(within(dlg).queryByRole("button", { name: "Reiniciar agente" })).toBeNull();
  });

  it("en Windows no hace falta ningún permiso", async () => {
    const { dlg } = await mount("demo&still&here=b", 2);
    expect(within(dlg).getByText("Esta máquina no necesita permisos")).toBeInTheDocument();
    expect(within(dlg).getByRole("button", { name: "Continuar" })).not.toHaveClass("wait");
  });
});

describe("paso 4: teclado y mouse", () => {
  it("cada máquina lista lo que tiene; la que no tiene nada lo dice", async () => {
    const { dlg } = await mount("demo&still", 3);
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(mac).getByText("Teclado integrado")).toBeInTheDocument();
    expect(within(mac).getByText("Trackpad")).toBeInTheDocument();
    expect(within(mac).getByText(/Keychron K3 · MX Master 3S/)).toBeInTheDocument();
    const pc = within(dlg).getByRole("group", { name: "PC-Escritorio" });
    expect(within(pc).getByText("Sin teclado ni mouse ahora")).toBeInTheDocument();
    expect(within(pc).getByText("Si conectas el hub aquí, lo detecta solo.")).toBeInTheDocument();
  });

  it("marca la actividad de cada máquina cuando la oye (agente y ventana)", async () => {
    const { dlg, source } = await mount("demo&still", 3);
    const pc = within(dlg).getByRole("group", { name: "PC-Escritorio" });
    expect(within(pc).queryByText("Actividad detectada")).toBeNull();
    source.heardKey("b");
    expect(await within(pc).findByText("Actividad detectada")).toBeInTheDocument();
    const mac = within(dlg).getByRole("group", { name: "MacBook Pro" });
    expect(within(mac).queryByText("Actividad detectada")).toBeNull();
    await fireEvent.keyDown(window, { key: "a" });
    expect(await within(mac).findByText("Actividad detectada")).toBeInTheDocument();
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
    expect(within(mac).queryByText("Actividad detectada")).toBeNull();
    const moved = new Event("pointermove");
    Object.assign(moved, { movementX: 3, movementY: 0 });
    window.dispatchEvent(moved);
    expect(await within(mac).findByText("Actividad detectada")).toBeInTheDocument();
  });
});

describe("paso 5: acomodo y prueba de cruce", () => {
  it("espera el cruce y, si pasa, lo dice y habilita Continuar", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    const next = within(dlg).getByRole("button", { name: "Continuar" });
    expect(next).toHaveClass("wait");
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    expect(await within(dlg).findByText(/Cruzó a PC-Escritorio por el borde derecho, como dice el acomodo. Vuelta medida: 0.4 ms./)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(true);
    expect(next).not.toHaveClass("wait");
  });

  it("por otro borde falla y se puede probar de nuevo", async () => {
    const { dlg, store } = await mount("demo&still&cross=wrong", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    expect(await within(dlg).findByText(/Cruzó por arriba, no por la derecha/)).toBeInTheDocument();
    expect(store.wizardCrossed).toBe(false);
    expect(within(dlg).getByRole("button", { name: "Probar de nuevo" })).toBeInTheDocument();
  });

  it("sin cruzar a tiempo, falla con el borde que había que usar", async () => {
    const { dlg } = await mount("demo&still&cross=timeout", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    expect(await within(dlg).findByText(/No cruzó en 20 s/)).toBeInTheDocument();
  });

  it("mover la otra máquina con las flechas guarda el acomodo y reinicia la prueba", async () => {
    const { dlg, store } = await mount("demo&still&cross=ok", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Probar cruce" }));
    await within(dlg).findByText(/Cruzó a/);
    const other = within(dlg).getByRole("button", { name: /Mover PC-Escritorio/ });
    other.focus();
    await userEvent.keyboard("{ArrowDown}");
    // A (aquí) sale por abajo: B queda debajo.
    expect(store.desk.layout.arrangement.aSide).toBe("up");
    expect(store.wizardCrossed).toBe(false);
    expect(within(dlg).getByRole("button", { name: "Probar cruce" })).toBeInTheDocument();
  });

  it("Saltar pasa al resumen sin la prueba", async () => {
    const { dlg, store } = await mount("demo&still", 4);
    await userEvent.click(within(dlg).getByRole("button", { name: "Saltar" }));
    expect(store.wizardStep).toBe(5);
    expect(within(dlg).getByText("cruce sin probar")).toBeInTheDocument();
  });
});

describe("paso 6: listo", () => {
  it("resume lo comprobado y copia el comando de MCP", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const { dlg, store } = await mount("demo&still", 5);
    store.wizardCrossed = true;
    await tick();
    const sum = within(dlg).getByRole("region", { name: "Resumen" });
    expect(sum).toHaveTextContent("MacBook Pro y PC-Escritorio en «Mi puesto»");
    expect(sum).toHaveTextContent("USB-C · 0.3 ms");
    expect(sum).toHaveTextContent("desde cualquier máquina · cruce probado");
    expect(sum).toHaveTextContent("MCP listo para Claude Code en las dos");
    expect(within(dlg).getByText(MCP_COMMAND)).toBeInTheDocument();
    await userEvent.click(within(dlg).getByRole("button", { name: "Copiar comando" }));
    expect(writeText).toHaveBeenCalledWith("claude mcp add tandem -- tandem mcp");
  });

  it("«Ir al puesto» cierra el asistente", async () => {
    const { dlg, store } = await mount("demo&still", 5);
    await userEvent.click(within(dlg).getByRole("button", { name: "Ir al puesto" }));
    expect(store.wizardOpen).toBe(false);
    expect(store.route).toEqual({ screen: "puesto" });
  });
});
