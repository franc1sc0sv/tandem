import { describe, expect, it } from "vitest";

import { bytesText, changeToast, deskEvents, KIND_NAME, TONE_COLOR, transferToast } from "./notices";
import { demoDesk, demoOptions } from "./store/demo";
import type { DeskView } from "./store/types";

const desk = (q = "demo&still"): DeskView => demoDesk(demoOptions(q));

describe("changeToast (D5)", () => {
  it("says who changed it, what changed and offers \"Undo\" back to the previous version", () => {
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
  it("without changes there are no notices, and without an agent neither", () => {
    expect(deskEvents(desk(), desk())).toEqual([]);
    const off = { ready: false } as unknown as DeskView;
    expect(deskEvents(off, desk())).toEqual([]);
    expect(deskEvents(desk(), off)).toEqual([]);
  });

  it("a cable that goes away warns with a warning tone and one that returns, with ok", () => {
    const usb = desk();
    const wifi = { ...usb, link: { ...usb.link, kind: "wifi" as const } };
    expect(deskEvents(usb, wifi)).toEqual([expect.objectContaining({ text: `The ${KIND_NAME.usb4} cable was unplugged`, tone: "warn" })]);
    expect(deskEvents(wifi, usb)).toEqual([expect.objectContaining({ text: `The ${KIND_NAME.usb4} cable is back`, tone: "ok" })]);
  });

  it("a new tunnel was opened by Claude; a closed or already known one does not notify", () => {
    const d = desk();
    const t0 = d.tunnels[0];
    const fresh = { ...t0, id: "fresh", title: "Test server", state: "busy" as const };
    const closed = { ...t0, id: "other", state: "closed" as const };
    const out = deskEvents(d, { ...d, tunnels: [...d.tunnels, fresh, closed] });
    expect(out).toHaveLength(1);
    expect(out[0]).toMatchObject({ text: "Claude opened the tunnel “Test server”", tone: "claude" });
  });

  it("the other machine going off, getting lost or coming back", () => {
    const d = desk();
    const peer = d.here === "a" ? "b" : "a";
    const withPower = (power: "on" | "off" | "lost"): DeskView => ({ ...d, machines: { ...d.machines, [peer]: { ...d.machines[peer], power } } }) as DeskView;
    const name = d.machines[peer]?.name;
    expect(deskEvents(withPower("on"), withPower("off"))[0]).toMatchObject({ text: `${name} went off`, tone: "warn" });
    expect(deskEvents(withPower("on"), withPower("lost"))[0]).toMatchObject({ text: `${name} was lost`, tone: "warn" });
    expect(deskEvents(withPower("off"), withPower("on"))[0]).toMatchObject({ text: `${name} is back`, tone: "claude" });
    expect(deskEvents(withPower("off"), withPower("lost"))).toEqual([]);
  });

  it("each tone has its color", () => {
    expect(Object.keys(TONE_COLOR).sort()).toEqual(["bad", "claude", "info", "ok", "warn"]);
  });
});

describe("transferToast (scope 3)", () => {
  const d = desk();
  const peer = d.machines[d.here === "a" ? "b" : "a"]?.name;

  it("while advancing it carries the bar, the percentage and the bytes, and does not go away by itself", () => {
    const t = transferToast({ upload: true, path: "/home/me/photos/wedding.zip", done: 500_000_000, total: 2_000_000_000, finished: false }, d);
    expect(t).toMatchObject({ key: "transfer:/home/me/photos/wedding.zip", text: `Sending “wedding.zip” to ${peer}`, body: "25 % · 500 MB of 2.0 GB", tone: "info" });
    expect(t.progress).toEqual({ done: 500_000_000, total: 2_000_000_000 });
  });

  it("when receiving it says so in the other direction, with the Windows path too", () => {
    const t = transferToast({ upload: false, path: "C:\\data\\notes.txt", done: 0, total: 0, finished: false }, d);
    expect(t.text).toBe(`Receiving “notes.txt” from ${peer}`);
    expect(t.body).toBe("0 % · 0 KB of 0 KB");
  });

  it("on finishing it is an ok notice with the same key and without a bar", () => {
    const run = transferToast({ upload: true, path: "/x/a.bin", done: 1, total: 2, finished: false }, d);
    const end = transferToast({ upload: true, path: "/x/a.bin", done: 2, total: 2, finished: true }, d);
    expect(end).toMatchObject({ key: run.key, text: "Copied “a.bin”", tone: "ok" });
    expect(end.progress).toBeUndefined();
  });

  it("bytesText picks the unit", () => {
    expect([bytesText(12_000), bytesText(320_000_000), bytesText(1_400_000_000)]).toEqual(["12 KB", "320 MB", "1.4 GB"]);
  });
});
