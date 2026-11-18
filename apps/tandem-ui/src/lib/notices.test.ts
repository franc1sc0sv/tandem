import { describe, expect, it } from "vitest";

import { bytesText, changeToast, deskEvents, KIND_NAME, TONE_COLOR, transferToast } from "./notices";
import { demoDesk, demoOptions } from "./store/demo";
import type { DeskView } from "./store/types";

const desk = (q = "demo&still"): DeskView => demoDesk(demoOptions(q));

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
    const nuevo = { ...t0, id: "nuevo", title: "Servidor de pruebas", state: "busy" as const };
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

describe("transferToast (alcance 3)", () => {
  const d = desk();
  const peer = d.machines[d.here === "a" ? "b" : "a"]?.name;

  it("mientras avanza trae la barra, el porcentaje y los bytes, y no se va solo", () => {
    const t = transferToast({ upload: true, path: "/home/yo/fotos/boda.zip", done: 500_000_000, total: 2_000_000_000, finished: false }, d);
    expect(t).toMatchObject({ key: "transfer:/home/yo/fotos/boda.zip", text: `Enviando «boda.zip» a ${peer}`, body: "25 % · 500 MB de 2.0 GB", tone: "info" });
    expect(t.progress).toEqual({ done: 500_000_000, total: 2_000_000_000 });
  });

  it("al recibir lo dice en la otra dirección, con la ruta de Windows también", () => {
    const t = transferToast({ upload: false, path: "C:\\datos\\notas.txt", done: 0, total: 0, finished: false }, d);
    expect(t.text).toBe(`Recibiendo «notas.txt» de ${peer}`);
    expect(t.body).toBe("0 % · 0 KB de 0 KB");
  });

  it("al terminar es un aviso ok con la misma clave y sin barra", () => {
    const run = transferToast({ upload: true, path: "/x/a.bin", done: 1, total: 2, finished: false }, d);
    const end = transferToast({ upload: true, path: "/x/a.bin", done: 2, total: 2, finished: true }, d);
    expect(end).toMatchObject({ key: run.key, text: "Se copió «a.bin»", tone: "ok" });
    expect(end.progress).toBeUndefined();
  });

  it("bytesText elige la unidad", () => {
    expect([bytesText(12_000), bytesText(320_000_000), bytesText(1_400_000_000)]).toEqual(["12 KB", "320 MB", "1.4 GB"]);
  });
});
