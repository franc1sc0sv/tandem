import { describe, expect, it } from "vitest";

import { bytesText, changeToast, deskEvents, KIND_NAME, TONE_COLOR, transferToast } from "./notices";
import { demoDesk, demoOptions } from "./store/demo";
import type { DeskView } from "./store/types";

const desk = (q = "demo&still"): DeskView => demoDesk(demoOptions(q));

describe("changeToast (D5)", () => {
  it("dice quién cambió, qué cambió y ofrece «Deshacer» hacia la versión anterior", () => {
    const d = desk();
    const undo: number[] = [];
    const t = changeToast({ by: "b", rev: 5, summary: "Screen arrangement", undoTo: 4 }, d, (r) => undo.push(r));
    expect(t).toMatchObject({ text: `Changed from ${d.machines.b?.name}`, body: "Screen arrangement", tone: "info" });
    expect(t.action?.label).toBe("Undo");
    t.action?.run();
    expect(undo).toEqual([4]);
  });
});

describe("deskEvents", () => {
  it("sin cambios no hay notices, y no agent tampoco", () => {
    expect(deskEvents(desk(), desk())).toEqual([]);
    const off = { ready: false } as unknown as DeskView;
    expect(deskEvents(off, desk())).toEqual([]);
    expect(deskEvents(desk(), off)).toEqual([]);
  });

  it("el cable que se va avisa con tono de advertencia y el que vuelve, en ok", () => {
    const usb = desk();
    const wifi = { ...usb, link: { ...usb.link, kind: "wifi" as const } };
    expect(deskEvents(usb, wifi)).toEqual([expect.objectContaining({ text: `The ${KIND_NAME.usb4} cable was unplugged`, tone: "warn" })]);
    expect(deskEvents(wifi, usb)).toEqual([expect.objectContaining({ text: `The ${KIND_NAME.usb4} cable is back`, tone: "ok" })]);
  });

  it("un tunnel fresh lo abrió Claude; uno closed o ya conocido no avisa", () => {
    const d = desk();
    const t0 = d.tunnels[0];
    const fresh = { ...t0, id: "fresh", title: "Test server", state: "busy" as const };
    const closed = { ...t0, id: "otro", state: "closed" as const };
    const out = deskEvents(d, { ...d, tunnels: [...d.tunnels, fresh, closed] });
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ text: "Claude opened the tunnel “Test server”", tone: "claude" });
  });

  it("the other machine que se apaga, se pierde o vuelve", () => {
    const d = desk();
    const peer = d.here === "a" ? "b" : "a";
    const con = (power: "on" | "off" | "lost"): DeskView => ({ ...d, machines: { ...d.machines, [peer]: { ...d.machines[peer], power } } }) as DeskView;
    const name = d.machines[peer]?.name;
    expect(deskEvents(con("on"), con("off"))[0]).toMatchObject({ text: `${name} went off`, tone: "warn" });
    expect(deskEvents(con("on"), con("lost"))[0]).toMatchObject({ text: `${name} was lost`, tone: "warn" });
    expect(deskEvents(con("off"), con("on"))[0]).toMatchObject({ text: `${name} is back`, tone: "claude" });
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
    const t = transferToast({ upload: true, path: "/home/me/photos/wedding.zip", done: 500_000_000, total: 2_000_000_000, finished: false }, d);
    expect(t).toMatchObject({ key: "transfer:/home/me/photos/wedding.zip", text: `Sending “wedding.zip” to ${peer}`, body: "25 % · 500 MB of 2.0 GB", tone: "info" });
    expect(t.progress).toEqual({ done: 500_000_000, total: 2_000_000_000 });
  });

  it("al recibir lo dice en la otra dirección, con la ruta de Windows también", () => {
    const t = transferToast({ upload: false, path: "C:\\data\\notes.txt", done: 0, total: 0, finished: false }, d);
    expect(t.text).toBe(`Receiving “notes.txt” from ${peer}`);
    expect(t.body).toBe("0 % · 0 KB of 0 KB");
  });

  it("al terminar es un notice ok con la misma clave y sin barra", () => {
    const run = transferToast({ upload: true, path: "/x/a.bin", done: 1, total: 2, finished: false }, d);
    const end = transferToast({ upload: true, path: "/x/a.bin", done: 2, total: 2, finished: true }, d);
    expect(end).toMatchObject({ key: run.key, text: "Copied “a.bin”", tone: "ok" });
    expect(end.progress).toBeUndefined();
  });

  it("bytesText elige la unidad", () => {
    expect([bytesText(12_000), bytesText(320_000_000), bytesText(1_400_000_000)]).toEqual(["12 KB", "320 MB", "1.4 GB"]);
  });
});
