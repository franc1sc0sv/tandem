import { describe, expect, it } from "vitest";

import {
  ASK_TIMEOUT_MS,
  abandonOnClose,
  keepInFlight,
  moveEnded,
  AskDeadline,
  STEPS,
  addAction,
  advanceTo,
  answerChoice,
  begin,
  canAdvance,
  candidateKey,
  candidateBlocker,
  candidateReady,
  countsAsKeyProbe,
  choiceOf,
  goBack,
  newMonitors,
  nextStep,
  shouldOpen,
  summaryChecks,
  verifyStep,
  homeOf,
  viewOf,
  type Onboarding,
  type StepId,
  type VerifyEntry,
} from "./onboarding";
import type { CapsMonitor, DeskState, InputView, MachineCaps, MonitorCandidate, MonitorConfig, MonitorScan, PairingView, PeerView, Snapshot } from "./model";

// ---------------------------------------------------------------------------------------
// El puesto real: cada monitor tiene un id PnP distinto por entrada.

const M34: MonitorConfig = {
  id: "sam711a",
  label: '34"',
  edid_model: "SAM711A",
  edid_aliases: ["SAM7119"],
  wiring: { pc: "displayport", mac: "hdmi" },
  ddc: { hdmi: 0x06, display_port: 0x09 },
};
const M32: MonitorConfig = {
  id: "sam7489",
  label: '32"',
  edid_model: "SAM7489",
  edid_aliases: ["SAM748A"],
  wiring: { pc: "displayport", mac: "hdmi" },
};

function caps(machine: "mac" | "pc", extra: Partial<MachineCaps> = {}): MachineCaps {
  const pc = machine === "pc";
  return {
    machine,
    name: pc ? "ESCRITORIO" : "MacBook-Pro",
    os: pc ? "windows" : "macos",
    scanned: true,
    monitors: [],
    builtin: !pc,
    hub: null,
    input: { capture: true, inject: true, missing: [] },
    power: { wake_peer: true, wakeable: true, power_off: pc },
    services: [],
    ...extra,
  };
}

const INPUT: InputView = { server: "mac", hub_here: null, session: true, locked: false, missing: [], problem: null, defined: false };
const PAIRED: PeerView = { kind: "paired", name: "PC", link: "usb4" };
const IDLE: PairingView = { kind: "idle" };

interface Parts {
  agent: "mac" | "pc";
  monitors: MonitorConfig[];
  peer: PeerView;
  pairing: PairingView;
  here: MachineCaps;
  state: DeskState;
}

function snap(over: Partial<Parts> = {}): Snapshot {
  const agent = over.agent ?? "mac";
  const monitors = over.monitors ?? [];
  const state: DeskState = over.state ?? {
    owner: Object.fromEntries(monitors.map((m) => [m.id, "mac"])),
    on: { mac: true, pc: true },
    builtin_pref: "off",
    doubt: Object.fromEntries(monitors.map((m) => [m.id, "unknown"])),
  };
  return {
    kind: "connected",
    agent,
    desk: { monitors },
    hidden: [],
    state,
    peer: over.peer ?? PAIRED,
    pairing: over.pairing ?? IDLE,
    input: INPUT,
    caps: { here: over.here ?? caps(agent), peer: caps(agent === "mac" ? "pc" : "mac") },
  };
}

const seen = (pnp: string, model: string, inches: number, extra: Partial<CapsMonitor> = {}): CapsMonitor => ({
  monitor: null,
  pnp_id: pnp,
  name: `${inches}"`,
  switch: false,
  ddc: false,
  seen: true,
  model,
  serial: null,
  inches,
  output: null,
  ...extra,
});

/** Lo que ve el asistente en la Mac con la PC enlazada. */
function candidate(over: Partial<MonitorCandidate> = {}): MonitorCandidate {
  return {
    label: '34"',
    pnp_ids: ["SAM711A", "SAM7119"],
    here: seen("SAM7119", "LC34G55T", 34, { serial: 7 }),
    peer: seen("SAM711A", "LC34G55T", 34, { serial: 7, output: "displayport" }),
    basis: "Serial",
    wiring: { mac: null, pc: "displayport" },
    in_desk: null,
    profile: "LC34G55T",
    ...over,
  };
}

const c32 = (): MonitorCandidate =>
  candidate({
    label: '32"',
    pnp_ids: ["SAM7489", "SAM748A"],
    here: seen("SAM748A", "Odyssey G5", 32, { serial: 9 }),
    peer: seen("SAM7489", "Odyssey G5", 32, { serial: 9, output: "displayport" }),
    profile: null,
  });

const scan = (candidates: MonitorCandidate[], extra: Partial<MonitorScan> = {}): MonitorScan => ({ candidates, peer_known: true, disconnected: 0, ...extra });

function at(step: StepId, patch: Partial<Onboarding> = {}): Onboarding {
  return { ...begin(step), ...patch };
}

const withScan = (s: MonitorScan, choices: Onboarding["monitors"]["choices"] = {}): Onboarding =>
  at("monitors", { monitors: { ...begin().monitors, scan: s, choices } });

/** Contesta todas las preguntas de un monitor ligado: vínculo sí, PC por DisplayPort, Mac por HDMI. */
function answered(o: Onboarding, c: MonitorCandidate): Onboarding {
  const k = candidateKey(c);
  let n = o;
  for (const id of [`link:${k}:yes`, `wire:${k}:pc:displayport`, `wire:${k}:mac:hdmi`]) n = answerChoice(n, id)!;
  return n;
}

const ids = (v: { buttons: { id: string }[] }) => v.buttons.map((b) => b.id);
const btn = (v: { buttons: { id: string; enabled: boolean }[] }, id: string) => v.buttons.find((b) => b.id === id);

// ---------------------------------------------------------------------------------------

describe("los pasos", () => {
  it("son siete y en este orden", () => {
    expect(STEPS.map((s) => s.id)).toEqual(["machine", "pair", "keyboard", "monitors", "verify", "initial", "summary"]);
  });

  it("se abre solo con el puesto vacío, y no con monitores o con alguno sin conectar", () => {
    expect(shouldOpen(snap())).toBe(true);
    expect(shouldOpen(snap({ monitors: [M34] }))).toBe(false);
    const hidden = snap();
    if (hidden.kind === "connected") hidden.hidden = [M34];
    expect(shouldOpen(hidden)).toBe(false);
    expect(shouldOpen({ kind: "connecting" })).toBe(false);
  });
});

describe("paso 1: esta máquina", () => {
  it("dice cómo se detectó la máquina", () => {
    const v = viewOf(at("machine"), snap({ agent: "pc" }));
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "PC · ESCRITORIO", detail: expect.stringContaining("Windows") });
    expect(v.verified).toBe(true);
  });

  it("en la Mac verifica los permisos en vivo: mientras falte uno no se avanza", () => {
    const missing = caps("mac", { input: { capture: false, inject: false, missing: ["accessibility", "input_monitoring"] } });
    const v = viewOf(at("machine"), snap({ here: missing }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("faltan permisos de macOS");
    expect(ids(v)).toEqual(["perm:accessibility", "perm:input_monitoring"]);
    expect(v.checks.filter((c) => c.state === "fail")).toHaveLength(2);
    // Se concede uno: el otro sigue.
    const one = caps("mac", { input: { capture: true, inject: true, missing: ["input_monitoring"] } });
    expect(ids(viewOf(at("machine"), snap({ here: one })))).toEqual(["perm:input_monitoring"]);
    // Se conceden todos: se comprueba solo.
    const ok = viewOf(at("machine"), snap());
    expect(ok.verified).toBe(true);
    expect(ok.checks.some((c) => c.label === "Permisos de macOS" && c.state === "ok")).toBe(true);
  });

  it("sin agente no hay nada que verificar", () => {
    const v = viewOf(at("machine"), { kind: "connecting" });
    expect(v.verified).toBe(false);
    expect(v.blocker).toContain("tandemd");
  });
});

describe("paso 2: la otra máquina", () => {
  const unpaired: PeerView = { kind: "unpaired" };

  it("sin emparejar se puede buscar o seguir solo por ahora", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired }));
    expect(ids(v)).toEqual(["pair:start", "pair:skip"]);
    expect(v.verified).toBe(false);
    const skipped = viewOf(at("pair", { pair: { ...begin().pair, skipped: true } }), snap({ peer: unpaired }));
    expect(skipped.verified).toBe(true);
    expect(skipped.checks.some((c) => c.label.includes("solo con esta máquina"))).toBe(true);
  });

  it("muestra el código y pide compararlo; no se puede saltar a mitad", () => {
    const comparing: PairingView = { kind: "comparing", code: "418027", peer: "PC", confirmed: false };
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: comparing }));
    expect(v.code).toBe("418027");
    expect(ids(v)).toEqual(["pair:confirm", "pair:cancel", "pair:skip"]);
    expect(btn(v, "pair:skip")?.enabled).toBe(false);
    const waiting = viewOf(at("pair"), snap({ peer: unpaired, pairing: { ...comparing, confirmed: true } }));
    expect(ids(waiting)).not.toContain("pair:confirm");
    const searching = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "searching" } }));
    expect(ids(searching)).toContain("pair:cancel");
  });

  it("un emparejamiento fallido dice por qué", () => {
    const v = viewOf(at("pair"), snap({ peer: unpaired, pairing: { kind: "failed", reason: "se rechazó en la PC" } }));
    expect(v.checks[0]).toMatchObject({ state: "fail", detail: "se rechazó en la PC" });
  });

  it("emparejada pero sin verificar el enlace no avanza: hace falta el ping", () => {
    const v = viewOf(at("pair"), snap());
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("prueba el enlace");
    expect(btn(v, "ping")).toMatchObject({ enabled: true, label: "Probar el enlace" });
    const done = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap());
    expect(done.verified).toBe(true);
    expect(done.checks.find((c) => c.label === "Ping del enlace")).toMatchObject({ state: "ok", detail: "Wi-Fi · 7 ms" });
  });

  it("emparejada pero sin enlace no se puede verificar", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, ping: { link: "wifi", millis: 7 } } }), snap({ peer: { kind: "paired", name: "PC", link: null } }));
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("la otra máquina no responde");
  });

  it("un ping fallido se ve", () => {
    const v = viewOf(at("pair", { pair: { ...begin().pair, pingError: "ningún enlace ve a la otra máquina" } }), snap());
    expect(v.checks.find((c) => c.label === "Ping del enlace")).toMatchObject({ state: "fail" });
  });
});

describe("paso 3: teclado y mouse", () => {
  it("al principio no hay nada definido y nunca se supone la Mac", () => {
    const v = viewOf(at("keyboard"), snap());
    expect(v.verified).toBe(false);
    expect(ids(v)).toEqual(["key:start", "key:pick:pc", "key:pick:mac", "key:skip"]);
    expect(v.checks.some((c) => c.detail === "nunca se supone que está en la Mac")).toBe(true);
  });

  it("escucha en las dos máquinas mientras espera la tecla", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, armed: true } }), snap());
    expect(v.checks[0]).toMatchObject({ state: "wait" });
    expect(v.checks[0]!.label).toContain("las dos máquinas");
    expect(ids(v)).not.toContain("key:start");
  });

  it("la máquina que recibió la tecla la tiene, pero hay que confirmarlo", () => {
    const heard = at("keyboard", { keyboard: { ...begin().keyboard, armed: true, heard: "pc" } });
    const v = viewOf(heard, snap());
    expect(v.checks[0]!.label).toBe("La tecla llegó a la PC");
    expect(ids(v)).toEqual(["key:yes:pc", "key:yes:mac", "key:again"]);
    expect(v.verified).toBe(false);
    expect(v.blocker).toBe("confirma dónde está el teclado");
  });

  it("confirmado queda verificado", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, confirmed: "pc" } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks[0]).toMatchObject({ state: "ok", label: "El teclado y el mouse están en la PC" });
  });

  it("sin respuesta se deja explícitamente sin definir", () => {
    const v = viewOf(at("keyboard", { keyboard: { ...begin().keyboard, undefinedOk: true } }), snap());
    expect(v.verified).toBe(true);
    expect(v.checks.some((c) => c.label === "Sin definir")).toBe(true);
  });

  it("sin la otra máquina enlazada avisa que solo se escucha aquí", () => {
    const v = viewOf(at("keyboard"), snap({ peer: { kind: "unpaired" } }));
    expect(v.checks[0]!.detail).toContain("solo se escucha aquí");
  });
});

describe("paso 4: monitores", () => {
  it("lista los conectados con las dos identidades y pide confirmar el vínculo", () => {
    const o = withScan(scan([candidate(), c32()]));
    const v = viewOf(o, snap());
    expect(v.rows).toHaveLength(2);
    const row = v.rows[0]!;
    expect(row.title).toBe('34"');
    expect(row.subtitle).toBe("SAM711A + SAM7119");
    expect(row.checks.map((c) => c.label)).toContain("Mac: SAM7119 (LC34G55T)");
    expect(row.checks.map((c) => c.label)).toContain("PC: SAM711A (LC34G55T)");
    const link = row.questions[0]!;
    expect(link.text).toContain("¿Este monitor de la Mac es este de la PC?");
    expect(link.text).toContain("mismo número de serie");
    // Con el mismo número de serie solo se sugiere: no viene contestado.
    expect(link.options.find((o) => o.id.endsWith(":yes"))!.label).toContain("sugerido");
    expect(link.options.every((o) => !o.selected)).toBe(true);
    expect(choiceOf(o, candidate()).linked).toBeNull();
  });

  it("por modelo o tamaño el vínculo no se supone: hay que contestar", () => {
    const model = candidate({ basis: "Model" });
    const o = withScan(scan([model]));
    expect(choiceOf(o, model).linked).toBeNull();
    const row = viewOf(o, snap()).rows[0]!;
    expect(row.questions[0]!.text).toContain("coinciden en modelo o tamaño");
    expect(row.questions).toHaveLength(1);
    expect(candidateReady(o, model, "mac")).toBe(false);
    // Contestado, aparecen las preguntas del cableado.
    const yes = answerChoice(o, `link:${candidateKey(model)}:yes`)!;
    expect(viewOf(yes, snap()).rows[0]!.questions.length).toBe(3);
  });

  it("el cableado se deduce de por dónde lo ve cada máquina y se pregunta lo que no se sabe", () => {
    const c = candidate();
    // El cable se pregunta después de confirmar el vínculo.
    const o = answerChoice(withScan(scan([c])), `link:${candidateKey(c)}:yes`)!;
    const row = viewOf(o, snap()).rows[0]!;
    // La PC lo ve por DisplayPort (detectado); de la Mac no se sabe.
    const pc = row.questions.find((q) => q.text.startsWith("PC:"))!;
    expect(pc.text).toContain("DisplayPort (detectado)");
    // Detectado = sugerido, no elegido.
    expect(pc.options.every((x) => !x.selected)).toBe(true);
    expect(pc.options.find((x) => x.id.endsWith(":displayport"))!.label).toContain("sugerido");
    const mac = row.questions.find((q) => q.text.startsWith("¿Por qué cable va a la Mac?"))!;
    expect(mac.options.every((x) => !x.selected)).toBe(true);
    expect(candidateReady(o, c, "mac")).toBe(false);
    // Lo detectado no alcanza: cada pregunta pide su pulsación.
    const macOnly = answerChoice(o, `wire:${candidateKey(c)}:mac:hdmi`)!;
    expect(candidateReady(macOnly, c, "mac")).toBe(false);
    const link = answerChoice(macOnly, `link:${candidateKey(c)}:yes`)!;
    expect(candidateReady(link, c, "mac")).toBe(false);
    const done = answerChoice(link, `wire:${candidateKey(c)}:pc:displayport`)!;
    expect(candidateReady(done, c, "mac")).toBe(true);
  });

  it("dos máquinas en la misma entrada no es un cableado válido", () => {
    const c = candidate();
    const o = answerChoice(withScan(scan([c])), `wire:${candidateKey(c)}:mac:displayport`)!;
    expect(candidateReady(o, c, "mac")).toBe(false);
  });

  // T-089: el botón deshabilitado decía «responde las preguntas de esta fila» aunque todo
  // estuviera contestado y la causa fuera el cable igual en las dos máquinas.
  it("dice el motivo concreto de por qué no se puede agregar", () => {
    const c = candidate();
    const key = candidateKey(c);
    let o = answerChoice(withScan(scan([c])), `link:${key}:yes`)!;
    o = answerChoice(o, `wire:${key}:mac:displayport`)!;
    expect(candidateBlocker(o, c, "mac")).toBe("falta confirmar por qué cable va a la PC");
    o = answerChoice(o, `wire:${key}:pc:displayport`)!;
    const why = candidateBlocker(o, c, "mac")!;
    expect(why).toContain("no pueden ir al mismo cable (DisplayPort en las dos)");
    const v = viewOf(o, snap());
    expect(btn(v, "add")).toMatchObject({ hint: `34": ${why}` });
    expect(v.rows[0]!.buttons[0]!.hint).toBe(why);
    o = answerChoice(o, `wire:${key}:mac:hdmi`)!;
    expect(candidateBlocker(o, c, "mac")).toBeNull();
  });

  it("agrega un monitor con las dos identidades y el cableado confirmado", () => {
    const c = candidate();
    const o = answered(withScan(scan([c])), c);
    expect(newMonitors(o, "mac")).toEqual([
      { label: '34"', pnp_ids: ["SAM711A", "SAM7119"], wiring: { mac: "hdmi", pc: "displayport" } },
    ]);
    const v = viewOf(o, snap());
    expect(btn(v, "add")).toMatchObject({ label: "Agregar 1 monitor", enabled: true });
    expect(addAction(o, "mac")).toEqual({ kind: "add_monitors", monitors: newMonitors(o, "mac") });
  });

  it("no se puede agregar con respuestas sin contestar", () => {
    const v = viewOf(withScan(scan([candidate()])), snap());
    expect(btn(v, "add")).toMatchObject({ enabled: false, hint: '34": falta confirmar si el monitor de la PC y el de la Mac son el mismo' });
    expect(newMonitors(withScan(scan([candidate()])), "mac")).toEqual([]);
  });

  it("si no son el mismo monitor, cada identidad queda como el suyo", () => {
    const c = candidate({ basis: "Model" });
    let o = withScan(scan([c]));
    const key = candidateKey(c);
    o = answerChoice(o, `link:${key}:no`)!;
    o = answerChoice(o, `wire:${key}:mac:hdmi`)!;
    o = answerChoice(o, `wire:${key}:pc:displayport`)!;
    const got = newMonitors(o, "mac");
    expect(got).toHaveLength(2);
    expect(got.map((m) => m.pnp_ids)).toEqual([["SAM7119"], ["SAM711A"]]);
    expect(got[0]!.wiring).toEqual({ mac: "hdmi" });
    expect(got[1]!.wiring).toEqual({ pc: "displayport" });
  });

  it("lo que ya está en el puesto no se vuelve a agregar", () => {
    const c = candidate({ in_desk: "sam711a" });
    const o = withScan(scan([c]));
    expect(newMonitors(o, "mac")).toEqual([]);
    expect(viewOf(o, snap()).rows[0]!.checks[0]).toMatchObject({ label: "Ya está en el puesto" });
  });

  it("el usuario elige cuáles agregar", () => {
    const [a, b] = [candidate(), c32()];
    let o = withScan(scan([a, b]));
    for (const c of [a, b]) o = answered(o, c);
    expect(newMonitors(o, "mac")).toHaveLength(2);
    o = answerChoice(o, `add:${candidateKey(b)}`)!;
    expect(newMonitors(o, "mac").map((m) => m.label)).toEqual(['34"']);
    expect(viewOf(o, snap()).buttons.find((x) => x.id === "add")!.label).toBe("Agregar 1 monitor");
  });

  it("sin la otra máquina solo ve los suyos y lo dice", () => {
    const solo = candidate({ peer: null, basis: "Single", pnp_ids: ["SAM7119"], wiring: { mac: null } });
    const o = withScan(scan([solo], { peer_known: false }));
    const v = viewOf(o, snap({ peer: { kind: "unpaired" } }));
    expect(v.checks.some((c) => c.label === "Solo se ven los de esta máquina")).toBe(true);
    // Un solo lado: no hay vínculo que confirmar, solo el cable.
    expect(v.rows[0]!.questions).toHaveLength(1);
    const o2 = answerChoice(o, `wire:${candidateKey(solo)}:mac:hdmi`)!;
    expect(newMonitors(o2, "mac")).toEqual([{ label: '34"', pnp_ids: ["SAM7119"], wiring: { mac: "hdmi" } }]);
  });

  it("avanza solo con monitores en el puesto, o seguido sin ellos a propósito", () => {
    expect(viewOf(withScan(scan([])), snap()).verified).toBe(false);
    expect(viewOf(withScan(scan([])), snap()).checks.some((c) => c.label === "Ningún monitor conectado")).toBe(true);
    expect(viewOf(withScan(scan([])), snap({ monitors: [M34] })).verified).toBe(true);
    const skipped = at("monitors", { monitors: { ...begin().monitors, scan: scan([]), skipped: true } });
    expect(viewOf(skipped, snap()).verified).toBe(true);
  });

  it("avisa de los monitores del puesto que ya no están conectados", () => {
    const v = viewOf(withScan(scan([], { disconnected: 1 })), snap({ monitors: [M34] }));
    expect(v.checks.map((c) => c.label)).toContain("1 monitor del puesto no está conectado");
  });

  it("un error de la búsqueda se muestra", () => {
    const o = at("monitors", { monitors: { ...begin().monitors, error: "sin Quartz" } });
    expect(viewOf(o, snap()).checks[0]).toMatchObject({ state: "fail", detail: "sin Quartz" });
  });
});

describe("paso 5: verificar cada monitor", () => {
  const home = (phase: VerifyEntry["phase"], h: "mac" | "pc" = "mac"): VerifyEntry => ({ home: h, phase });
  const run = (e: VerifyEntry, ...events: Parameters<typeof verifyStep>[2][]) => {
    const effects: ReturnType<typeof verifyStep>["effects"] = [];
    for (const ev of events) {
      const r = verifyStep("m", e, ev);
      e = r.next;
      effects.push(...r.effects);
    }
    return { entry: e, effects };
  };

  it("la ida y vuelta termina verificada si se ve en las dos máquinas", () => {
    const { entry, effects } = run(
      home({ kind: "idle" }),
      { type: "start" },
      { type: "moved", ok: true },
      { type: "answer", yes: true },
      { type: "moved", ok: true },
      { type: "answer", yes: true },
    );
    expect(entry.phase).toEqual({ kind: "ok" });
    // Va a la PC y vuelve a la Mac por sí solo; no hace falta marcarlo manual.
    expect(effects).toEqual([
      { kind: "move", monitor: "m", to: "pc", back: "mac" },
      { kind: "move", monitor: "m", to: "mac", back: "mac" },
    ]);
  });

  it("si no se ve la otra máquina vuelve solo y queda en modo manual asistido", () => {
    const { entry, effects } = run(
      home({ kind: "idle" }),
      { type: "start" },
      { type: "moved", ok: true },
      { type: "answer", yes: false },
      { type: "moved", ok: true },
    );
    expect(entry.phase).toEqual({ kind: "manual", reason: "no se vio la PC" });
    expect(effects).toEqual([
      { kind: "move", monitor: "m", to: "pc", back: "mac" },
      { kind: "move", monitor: "m", to: "mac", back: "mac" },
      { kind: "mark_manual", monitor: "m", discard: true },
    ]);
  });

  it("si no se ve la vuelta también queda manual", () => {
    const { entry } = run(
      home({ kind: "idle" }, "pc"),
      { type: "start" },
      { type: "moved", ok: true },
      { type: "answer", yes: true },
      { type: "moved", ok: true },
      { type: "answer", yes: false },
    );
    expect(entry.phase).toMatchObject({ kind: "manual", reason: "no se vio la PC al volver" });
  });

  it("si no se pudo pasar, no hay nada que volver y queda manual con el motivo, sin tirar el perfil", () => {
    const { entry, effects } = run(home({ kind: "idle" }), { type: "start" }, { type: "moved", ok: false, reason: "la PC no responde" });
    expect(entry.phase).toEqual({ kind: "manual", reason: "la PC no responde" });
    expect(effects.map((e) => e.kind)).toEqual(["move", "mark_manual"]);
    // Un fallo de transporte no prueba nada contra los códigos (T-089).
    expect(effects[1]).toMatchObject({ discard: false });
  });

  it("si falla la vuelta tampoco se tira el perfil", () => {
    const { effects } = run(
      home({ kind: "idle" }),
      { type: "start" },
      { type: "moved", ok: true },
      { type: "answer", yes: true },
      { type: "moved", ok: false },
    );
    expect(effects.at(-1)).toMatchObject({ kind: "mark_manual", discard: false });
  });

  it("si falla la vuelta se dice", () => {
    const { entry } = run(
      home({ kind: "idle" }),
      { type: "start" },
      { type: "moved", ok: true },
      { type: "answer", yes: true },
      { type: "moved", ok: false },
    );
    expect(entry.phase).toMatchObject({ kind: "manual", reason: "no se pudo volver a la Mac" });
  });

  it("ignora respuestas y avisos fuera de turno", () => {
    const idle = home({ kind: "idle" });
    expect(verifyStep("m", idle, { type: "answer", yes: true }).next).toBe(idle);
    expect(verifyStep("m", idle, { type: "moved", ok: true }).next).toBe(idle);
    const moving = home({ kind: "moving", to: "pc" });
    expect(verifyStep("m", moving, { type: "start" }).next).toBe(moving);
    expect(verifyStep("m", home({ kind: "ok" }), { type: "start" }).next.phase.kind).toBe("ok");
    // Un monitor manual se puede volver a probar.
    expect(verifyStep("m", home({ kind: "manual", reason: "x" }), { type: "start" }).next.phase.kind).toBe("moving");
  });

  const two = snap({ monitors: [M34, M32] });

  // Con el estado sin confirmar (el del asistente) no se sabe dónde está cada monitor.
  const said = (id: string, h: "mac" | "pc" = "mac"): Partial<Onboarding> => ({ verify: { [id]: { home: h, phase: { kind: "idle" } } } });

  it("cada monitor muestra su perfil; con estado sin confirmar primero se pregunta dónde está", () => {
    const v = viewOf(at("verify"), two);
    expect(v.rows.map((r) => r.title)).toEqual(['34"', '32"']);
    expect(v.rows[0]!.checks[0]).toMatchObject({ state: "ok", label: "Perfil DDC", detail: "DP 0x09 · HDMI 0x06" });
    expect(v.rows[1]!.checks[0]).toMatchObject({ state: "idle", label: "Sin perfil DDC" });
    // Sin suponer la Mac: no hay botón de probar hasta que el usuario lo diga.
    expect(v.rows[0]!.buttons).toEqual([]);
    expect(v.rows[0]!.questions[0]!.text).toBe('¿Qué ves ahora en el 34"?');
    expect(v.rows[0]!.questions[0]!.options.map((x) => x.id)).toEqual(["home:sam711a:pc", "home:sam711a:mac"]);
    expect(viewOf(at("verify", said("sam711a")), two).rows[0]!.buttons[0]).toMatchObject({ id: "verify:sam711a", label: "Probar la ida y vuelta", enabled: true });
    expect(v.verified).toBe(false);
  });

  it("homeOf: lo dicho manda; confirmado, el estado; sin confirmar y sin decir, nada", () => {
    const c = viewOf(at("verify"), two) && (two as Extract<typeof two, { kind: "connected" }>);
    expect(homeOf(at("verify"), c.state, "sam711a")).toBeNull();
    expect(homeOf(at("verify", said("sam711a", "pc")), c.state, "sam711a")).toBe("pc");
    expect(homeOf(at("verify"), { ...c.state, doubt: {} }, "sam711a")).toBe(c.state.owner["sam711a"]);
  });

  it("un monitor marcado manual sin saber dónde estaba no cuenta como «dicho»: al calibrar vuelve a preguntar", () => {
    const c = two as Extract<typeof two, { kind: "connected" }>;
    const filler = at("verify", { verify: { sam711a: { home: "mac", unknownHome: true, phase: { kind: "manual", reason: "sin perfil DDC" } } } });
    expect(homeOf(filler, c.state, "sam711a")).toBeNull();
    expect(viewOf(filler, two).rows[0]!.questions[0]!.text).toBe('¿Qué ves ahora en el 34"?');
  });

  it("sin perfil DDC no hay ida y vuelta automática: se calibra (solo en la PC) o se cambia a mano", () => {
    expect(viewOf(at("verify"), two).rows[1]!.buttons.map((b) => b.id)).toEqual(["manual:sam7489"]);
    const pc = snap({ agent: "pc", monitors: [M34, M32] });
    expect(viewOf(at("verify"), pc).rows[1]!.buttons.map((b) => b.id)).toEqual(["calibrate:sam7489", "manual:sam7489"]);
    // Ni aunque ya se sepa dónde está: antes va la calibración.
    expect(viewOf(at("verify", said("sam7489")), pc).rows[1]!.buttons.map((b) => b.id)).toEqual(["calibrate:sam7489", "manual:sam7489"]);
  });

  it("pregunta qué se ve, con botones de sí y no", () => {
    const o = at("verify", { verify: { sam711a: home({ kind: "ask", machine: "pc" }) } });
    const row = viewOf(o, two).rows[0]!;
    expect(row.questions[0]!.text).toBe('¿Ves la PC en el 34"?');
    expect(row.questions[0]!.options.map((x) => x.id)).toEqual(["ask:sam711a:yes", "ask:sam711a:no"]);
    expect(row.buttons).toEqual([]);
    const back = at("verify", { verify: { sam711a: home({ kind: "ask_home" }) } });
    expect(viewOf(back, two).rows[0]!.questions[0]!.text).toBe('¿Ves la Mac en el 34"?');
  });

  it("mientras uno se prueba, los demás esperan", () => {
    const o = at("verify", { verify: { sam711a: home({ kind: "moving", to: "pc" }) } });
    const v = viewOf(o, two);
    expect(v.rows[1]!.buttons[0]!.enabled).toBe(false);
    expect(v.rows[0]!.checks[1]!.label).toBe("Pasándolo a la PC…");
  });

  it("no se puede probar si la otra máquina está apagada o no hay cable hacia ella", () => {
    const off = snap({ monitors: [M34], state: { owner: { sam711a: "mac" }, on: { mac: true, pc: false }, builtin_pref: "off", doubt: {} } });
    expect(viewOf(at("verify"), off).rows[0]!.buttons[0]).toMatchObject({ enabled: false, hint: "la PC tiene que estar encendida" });
  });

  it("queda verificado cuando todos pasaron o quedaron en manual (dicho claramente)", () => {
    const o = at("verify", { verify: { sam711a: home({ kind: "ok" }), sam7489: home({ kind: "manual", reason: "no se vio la PC" }) } });
    const v = viewOf(o, two);
    expect(v.verified).toBe(true);
    expect(v.rows[1]!.checks.at(-1)).toMatchObject({ state: "fail", label: "Modo manual asistido" });
    expect(v.rows[1]!.checks.at(-1)!.detail).toContain("no se vio la PC");
    expect(v.rows[1]!.buttons.map((b) => b.id)).toEqual(["manual:sam7489"]);
    expect(viewOf(at("verify", { verify: { sam711a: home({ kind: "ok" }) } }), two).verified).toBe(false);
  });
});

describe("paso 6: estado inicial", () => {
  const one = snap({ monitors: [M34] });
  const confirmed = snap({
    monitors: [M34],
    state: { owner: { sam711a: "mac" }, on: { mac: true, pc: true }, builtin_pref: "off", doubt: {} },
  });

  it("elige un modo y lo aplica; antes de aplicar no está verificado", () => {
    const v = viewOf(at("initial"), one);
    expect(ids(v)).toEqual(["mode:mac", "mode:shared", "mode:pc", "apply"]);
    expect(btn(v, "apply")!.enabled).toBe(false);
    const picked = viewOf(at("initial", { initial: { ...begin().initial, mode: "shared" } }), one);
    expect(btn(picked, "apply")).toMatchObject({ enabled: true, label: "Aplicar modo" });
    expect(picked.checks[0]!.label).toBe("Modo elegido: Compartido");
    expect(picked.verified).toBe(false);
  });

  it("mientras aplica no deja cambiar nada", () => {
    const v = viewOf(at("initial", { initial: { mode: "mac", applying: true, applied: false, looksRight: null } }), one);
    expect(v.checks[1]).toMatchObject({ state: "wait", label: "Aplicando…" });
    expect(v.buttons.every((b) => !b.enabled)).toBe(true);
  });

  it("aplicado y confirmado pregunta si se ve bien; la respuesta verifica el paso", () => {
    const facts = { mode: "mac" as const, applying: false, applied: true, looksRight: null };
    const asking = viewOf(at("initial", { initial: facts }), confirmed);
    expect(asking.checks.at(-1)).toMatchObject({ state: "wait", label: "¿Se ve como esperas?" });
    expect(ids(asking)).toContain("looks:yes");
    expect(asking.verified).toBe(false);
    const ok = viewOf(at("initial", { initial: { ...facts, looksRight: true } }), confirmed);
    expect(ok.verified).toBe(true);
    const no = viewOf(at("initial", { initial: { ...facts, looksRight: false } }), confirmed);
    expect(no.verified).toBe(false);
    expect(no.checks.at(-1)).toMatchObject({ state: "fail" });
  });

  it("aplicado pero con el estado todavía sin confirmar no cuenta", () => {
    const v = viewOf(at("initial", { initial: { mode: "mac", applying: false, applied: true, looksRight: true } }), one);
    expect(v.verified).toBe(false);
  });
});

describe("paso 7: resumen", () => {
  it("junta todo lo verificado", () => {
    const o = at("summary", {
      pair: { ...begin().pair, ping: { link: "usb4", millis: 2 } },
      keyboard: { ...begin().keyboard, confirmed: "pc" },
      verify: { sam711a: { home: "mac", phase: { kind: "ok" } }, sam7489: { home: "mac", phase: { kind: "manual", reason: "x" } } },
      initial: { mode: "shared", applying: false, applied: true, looksRight: true },
    });
    const s = snap({
      monitors: [M34, M32],
      state: { owner: { sam711a: "pc", sam7489: "mac" }, on: { mac: true, pc: true }, builtin_pref: "off", doubt: {} },
    });
    const lines = summaryChecks(o, s);
    const text = lines.map((l) => `${l.state}|${l.label}|${l.detail ?? ""}`);
    expect(text).toContain("ok|Mac · MacBook-Pro|permisos verificados");
    expect(text).toContain("ok|PC emparejada|USB4 · 2 ms");
    expect(text).toContain("ok|Teclado y mouse en la PC|confirmado por ti");
    expect(text).toContain('ok|34" (SAM711A + SAM7119)|DP → PC · HDMI → Mac · perfil DDC · ida y vuelta verificada');
    expect(text).toContain('fail|32" (SAM7489 + SAM748A)|DP → PC · HDMI → Mac · sin perfil · modo manual asistido');
    expect(text).toContain("ok|Estado confirmado|modo Compartido");
    expect(viewOf(o, s).buttons.map((b) => b.id)).toEqual(["finish"]);
  });

  it("lo que no se hizo se dice: teclado sin definir, sin la otra máquina", () => {
    const lines = summaryChecks(begin("summary"), snap({ peer: { kind: "unpaired" } })).map((l) => l.label);
    expect(lines).toContain("Teclado y mouse sin definir");
    expect(lines).toContain("Sin la otra máquina");
  });
});

describe("navegación", () => {
  it("no avanza si el paso no está verificado", () => {
    const o = at("pair");
    expect(canAdvance(o, snap())).toBe(false);
    expect(advanceTo(o, snap())).toBe(o);
    const ok = at("pair", { pair: { ...begin().pair, ping: { link: "usb4", millis: 1 } } });
    expect(canAdvance(ok, snap())).toBe(true);
    expect(advanceTo(ok, snap()).step).toBe("keyboard");
  });

  it("sin monitores se saltan la prueba y el estado inicial", () => {
    expect(nextStep(at("monitors"), snap())).toBe("summary");
    expect(nextStep(at("monitors"), snap({ monitors: [M34] }))).toBe("verify");
    expect(goBack(at("summary"), snap()).step).toBe("monitors");
    expect(goBack(at("summary"), snap({ monitors: [M34] })).step).toBe("initial");
  });

  it("se puede volver atrás hasta el primero, y en el resumen ya no se avanza", () => {
    expect(goBack(at("machine"), snap()).step).toBe("machine");
    expect(goBack(at("keyboard"), snap()).step).toBe("pair");
    expect(canAdvance(at("summary"), snap())).toBe(false);
  });

  it("un recorrido completo con el puesto real pasa por los siete pasos", () => {
    let o = begin();
    const s = snap({ monitors: [M34, M32] });
    const visited: StepId[] = [o.step];
    const facts: Record<StepId, Partial<Onboarding>> = {
      machine: {},
      pair: { pair: { ...begin().pair, ping: { link: "usb4", millis: 3 } } },
      keyboard: { keyboard: { ...begin().keyboard, confirmed: "pc" } },
      monitors: {},
      verify: { verify: { sam711a: { home: "mac", phase: { kind: "ok" } }, sam7489: { home: "mac", phase: { kind: "ok" } } } },
      initial: { initial: { mode: "mac", applying: false, applied: true, looksRight: true } },
      summary: {},
    };
    const confirmedState = snap({ monitors: [M34, M32], state: { owner: { sam711a: "mac", sam7489: "mac" }, on: { mac: true, pc: true }, builtin_pref: "off", doubt: {} } });
    for (let i = 0; i < 6; i++) {
      o = { ...o, ...facts[o.step] };
      o = advanceTo(o, o.step === "initial" || o.step === "summary" ? confirmedState : s);
      visited.push(o.step);
    }
    expect(visited).toEqual(["machine", "pair", "keyboard", "monitors", "verify", "initial", "summary"]);
  });
});

describe("presionar una tecla", () => {
  it("cuenta cualquier tecla del teclado, pero no activar un botón del diálogo", () => {
    expect(countsAsKeyProbe({ key: "a", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "a", targetIsButton: true })).toBe(true);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: " ", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Tab", targetIsButton: true })).toBe(false);
    expect(countsAsKeyProbe({ key: "Enter", targetIsButton: false })).toBe(true);
    expect(countsAsKeyProbe({ key: "Shift", targetIsButton: false })).toBe(false);
    expect(countsAsKeyProbe({ key: "a", repeat: true, targetIsButton: false })).toBe(false);
  });
});

describe("paso 5: plazos y monitores de una sola máquina", () => {
  const lonely: MonitorConfig = { ...M32, wiring: { mac: "hdmi" } };

  it("un monitor cableado a una sola máquina no atasca el paso: queda dicho y verificado", () => {
    const s = snap({ monitors: [M34, lonely] });
    const v = viewOf(at("verify", { verify: { sam711a: { home: "mac", phase: { kind: "ok" } } } }), s);
    expect(v.verified).toBe(true);
    const row = v.rows[1]!;
    expect(row.buttons).toEqual([]);
    expect(row.checks.at(-1)).toMatchObject({ label: "Solo cableado a una máquina: sin ida y vuelta" });
    // Solo, también.
    expect(viewOf(at("verify"), snap({ monitors: [lonely] })).verified).toBe(true);
    expect(summaryChecks(at("summary"), s).some((l) => (l.detail ?? "").includes("sin ida y vuelta"))).toBe(true);
  });

  it("sin respuesta vuelve a donde estaba y queda en manual, sin tirar el perfil", () => {
    const asked: VerifyEntry = { home: "mac", phase: { kind: "ask", machine: "pc" } };
    const r = verifyStep("m", asked, { type: "timeout" });
    expect(r.next.phase).toMatchObject({ kind: "returning", silent: true });
    expect(r.effects).toEqual([{ kind: "move", monitor: "m", to: "mac", back: "mac" }]);
    // Una pregunta sin contestar no es un «no» del usuario: los códigos se conservan (T-089).
    const back = verifyStep("m", r.next, { type: "moved", ok: true });
    expect(back.next.phase.kind).toBe("manual");
    expect(back.effects).toEqual([{ kind: "mark_manual", monitor: "m", discard: false }]);
    // También al volver.
    const home: VerifyEntry = { home: "mac", phase: { kind: "ask_home" } };
    const late = verifyStep("m", home, { type: "timeout" });
    expect(late.next.phase.kind).toBe("manual");
    expect(late.effects).toEqual([{ kind: "mark_manual", monitor: "m", discard: false }]);
    // Fuera de turno no hace nada.
    const idle: VerifyEntry = { home: "mac", phase: { kind: "idle" } };
    expect(verifyStep("m", idle, { type: "timeout" }).next).toBe(idle);
  });

  /** Reloj simulado a mano. */
  function fake() {
    let now = 0;
    const timers = new Map<number, { at: number; fn: () => void }>();
    let n = 0;
    return {
      clock: {
        set: (fn: () => void, ms: number) => {
          timers.set(++n, { at: now + ms, fn });
          return n;
        },
        clear: (h: unknown) => void timers.delete(h as number),
      },
      advance(ms: number) {
        now += ms;
        for (const [id, t] of [...timers]) if (t.at <= now) {
          timers.delete(id);
          t.fn();
        }
      },
    };
  }

  it("el plazo vence a los 20 s de esperar una respuesta y solo una vez", () => {
    const { clock, advance } = fake();
    const expired: string[] = [];
    const d = new AskDeadline(clock, (m) => expired.push(m));
    d.track("m34", "moving");
    advance(60_000);
    expect(expired).toEqual([]);
    d.track("m34", "ask");
    d.track("m34", "ask");
    expect(d.pending).toBe(1);
    advance(ASK_TIMEOUT_MS - 1);
    expect(expired).toEqual([]);
    advance(1);
    expect(expired).toEqual(["m34"]);
    advance(60_000);
    expect(expired).toEqual(["m34"]);
  });

  it("contestar a tiempo desarma el plazo", () => {
    const { clock, advance } = fake();
    const expired: string[] = [];
    const d = new AskDeadline(clock, (m) => expired.push(m));
    d.track("m34", "ask");
    d.track("m34", "returning");
    advance(ASK_TIMEOUT_MS * 2);
    expect(expired).toEqual([]);
    expect(d.pending).toBe(0);
  });

  it("cerrar el diálogo a mitad de la prueba vence todo lo pendiente ya", () => {
    const { clock } = fake();
    const expired: string[] = [];
    const d = new AskDeadline(clock, (m) => expired.push(m));
    d.track("a", "ask");
    d.track("b", "ask_home");
    d.track("c", "moving");
    d.expireAll();
    expect(expired.sort()).toEqual(["a", "b"]);
    expect(d.pending).toBe(0);
  });
});

describe("cerrar el asistente a mitad del movimiento", () => {
  const moving = (home: "mac" | "pc" = "mac"): Record<string, VerifyEntry> => ({
    m34: { home, phase: { kind: "moving", to: home === "mac" ? "pc" : "mac" } },
    m32: { home: "mac", phase: { kind: "ask", machine: "pc" } },
  });

  it("solo marca lo que todavía se mueve, con su home", () => {
    const ab = abandonOnClose(moving("pc"), new Map());
    expect([...ab]).toEqual([["m34", "pc"]]);
  });

  it("al terminar el movimiento con la marca, vuelve directo a su home y la marca se borra", () => {
    const ab = abandonOnClose(moving(), new Map());
    const r = moveEnded(ab, "m34", "committed");
    expect(r.returnTo).toBe("mac");
    expect(r.abandoned.size).toBe(0);
    // Una segunda vez ya no hay nada que volver.
    expect(moveEnded(r.abandoned, "m34", "committed").returnTo).toBeNull();
  });

  it.each(["rolled_back", "inconsistent", "rejected"] as const)("si el movimiento termina en %s la marca se limpia y no vuelve nada", (end) => {
    const ab = abandonOnClose(moving(), new Map());
    const r = moveEnded(ab, "m34", end);
    expect(r.returnTo).toBeNull();
    expect(r.abandoned.size).toBe(0);
    // La siguiente prueba del monitor no se dispara sola.
    expect(moveEnded(r.abandoned, "m34", "committed").returnTo).toBeNull();
  });

  it("cerrar y reabrir antes del committed conserva la marca del movimiento en vuelo", () => {
    const ab = abandonOnClose(moving(), new Map());
    const reopened = keepInFlight(ab, "m34");
    expect(moveEnded(reopened, "m34", "committed").returnTo).toBe("mac");
    // Si ya no hay movimiento en vuelo, la marca vieja se descarta.
    expect(keepInFlight(ab, null).size).toBe(0);
    expect(keepInFlight(ab, "m32").size).toBe(0);
  });

  it("si la vuelta también falla estando de regreso, se dice que quizá no volvió", () => {
    const e: VerifyEntry = { home: "mac", phase: { kind: "returning", failed: "no se vio la PC" } };
    const r = verifyStep("m", e, { type: "moved", ok: false, reason: "la PC no responde" });
    expect(r.next.phase).toMatchObject({ kind: "manual" });
    const why = (r.next.phase as { reason: string }).reason;
    expect(why).toContain("no se vio la PC");
    expect(why).toContain("la PC no responde");
    expect(why).toContain("no haya vuelto");
  });
});
