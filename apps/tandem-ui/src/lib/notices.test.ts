import { describe, expect, it } from "vitest";

import { changeToast, deskEvents, KIND_NAME, TONE_COLOR } from "./notices";
import { demoDesk, demoOptions } from "./store/demo";
import type { DeskView } from "./store/types";

const desk = (q = "demo&still"): Extract<DeskView, { ready: true }> => demoDesk(demoOptions(q)) as Extract<DeskView, { ready: true }>;

describe("changeToast (D5)", () => {
  it("dice quién cambió, qué cambió y ofrece «Deshacer» hacia la versión anterior", () => {
    const d = desk();
    const undo: number[] = [];
    const t = changeToast({ by: "b", rev: 5, summary: "Acomodo de pantallas", undoTo: 4 }, d, (r) => undo.push(r));
    expect(t).toMatchObject({ text: `Cambiado desde ${d.machines.b?.name}`, body: "Acomodo de pantallas", tone: "info" });
    expect(t.action?.label).toBe("Deshacer");
    t.action?.run();
    expect(undo).toEqual([4]);
  });
});

describe("deskEvents", () => {
  it("sin cambios no hay avisos, y sin agente tampoco", () => {
    expect(deskEvents(desk(), desk())).toEqual([]);
    const off = { ready: false } as unknown as DeskView;
    expect(deskEvents(off, desk())).toEqual([]);
    expect(deskEvents(desk(), off)).toEqual([]);
  });

  it("el cable que se va avisa con tono de advertencia y el que vuelve, en ok", () => {
    const usb = desk();
    const wifi = { ...usb, link: { ...usb.link, kind: "wifi" as const } };
    expect(deskEvents(usb, wifi)).toEqual([expect.objectContaining({ text: `Se desconectó el cable ${KIND_NAME.usb4}`, tone: "warn" })]);
    expect(deskEvents(wifi, usb)).toEqual([expect.objectContaining({ text: `Volvió el cable ${KIND_NAME.usb4}`, tone: "ok" })]);
  });

  it("un túnel nuevo lo abrió Claude; uno cerrado o ya conocido no avisa", () => {
    const d = desk();
    const t0 = d.tunnels[0];
    const nuevo = { ...t0, id: "nuevo", title: "Servidor de pruebas", state: "open" as const };
    const cerrado = { ...t0, id: "otro", state: "closed" as const };
    const out = deskEvents(d, { ...d, tunnels: [...d.tunnels, nuevo, cerrado] });
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ text: "Claude abrió el túnel «Servidor de pruebas»", tone: "claude" });
  });

  it("la otra máquina que se apaga, se pierde o vuelve", () => {
    const d = desk();
    const peer = d.here === "a" ? "b" : "a";
    const con = (power: "on" | "off" | "lost"): DeskView => ({ ...d, machines: { ...d.machines, [peer]: { ...d.machines[peer], power } } }) as DeskView;
    const name = d.machines[peer]?.name;
    expect(deskEvents(con("on"), con("off"))[0]).toMatchObject({ text: `${name} se apagó`, tone: "warn" });
    expect(deskEvents(con("on"), con("lost"))[0]).toMatchObject({ text: `${name} se perdió`, tone: "warn" });
    expect(deskEvents(con("off"), con("on"))[0]).toMatchObject({ text: `${name} volvió`, tone: "claude" });
    expect(deskEvents(con("off"), con("lost"))).toEqual([]);
  });

  it("cada tono tiene su color", () => {
    expect(Object.keys(TONE_COLOR).sort()).toEqual(["bad", "claude", "info", "ok", "warn"]);
  });
});
