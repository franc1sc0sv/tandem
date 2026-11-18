// El estado de la ventana: el puesto (de la fuente real o del demo), la pantalla visible, la
// paleta y los avisos. Las pantallas leen de aquí y piden acciones aquí.

import type { CommandContext } from "../commands";
import { shouldOpen as pairingShouldOpen } from "../pairing";
import type { Machine, MonitorInput, SceneJson } from "../model";
import { feedRun, running, viewOf, type Names, type Run, type RunView } from "../txn";
import { machineName } from "../machines";
import type { Bindings, Permission, ShortcutsView } from "../model";
import { HOME, SCREEN_TOPICS, formatHash, parseHash, sameRoute, type Route } from "../routes";
import { EMPTY_DESK, deskFromError } from "./live";
import type { ActResult, Capability, DeskView, LinkKind, PairActionKind, Source, Topic } from "./types";
import type { UpdateView } from "../updates";
import type { Arrangement } from "../acomodo";
import type { CrossSample } from "../crossing";

/** Pasos del asistente que se abren desde otras partes (índices de `WIZARD_STEPS`). */
export const WIZARD_PAIR = 1;
export const WIZARD_PERMISSIONS = 2;

/** Lo que el shell muestra siempre (barra lateral: máquinas y contador de túneles). */
export const SHELL_TOPICS: Topic[] = ["machines", "tunnels"];

/** La bandeja: todo lo que lista, sin pantallas. */
export const TRAY_TOPICS: Topic[] = ["machines", "link", "scenes", "tunnels", "resources"];

/** Temas del shell más los de la pantalla, sin repetir. */
export function topicsFor(route: Route): Topic[] {
  return [...new Set([...SHELL_TOPICS, ...SCREEN_TOPICS[route.screen]])];
}

export interface Toast {
  id: number;
  text: string;
  tone: "ok" | "bad";
}

const TOAST_MS = 4000;

export class Store implements CommandContext {
  desk: DeskView = $state(EMPTY_DESK);
  route: Route = $state(HOME);
  paletteOpen = $state(false);
  wizardOpen = $state(false);
  toasts: Toast[] = $state([]);
  /** La transacción que se está viendo o acaba de terminar (escena, cambio de entrada). */
  run: Run | null = $state(null);
  /** El encendido de una máquina (otra tarjeta de «Cambios en progreso»). */
  powerRun: Run | null = $state(null);
  /** El último enlace que se midió desde esta ventana (su tarjeta queda a la vista). */
  measured: LinkKind | null = $state(null);
  /** Reloj de la pantalla de progreso (cuenta el tiempo mientras corre). */
  clock = $state(Date.now());
  /** Ya se sabe si hay avisos de transacciones: antes, «sin progreso» todavía no quiere decir nada. */
  txnReady = $state(false);
  /** Número de túneles activos, para la barra lateral. */
  activeTunnels = $derived(this.desk.tunnels.filter((t) => t.state === "busy").length);

  private nextToast = 1;
  /** Lo que se pidió desde esta ventana: da el título y el texto final de la transacción que arranque. */
  private origin: { title: string; doneText: string } | null = null;
  private stopTxn: (() => void) | null = null;
  private clockTimer: ReturnType<typeof setInterval> | undefined;

  /**
   * `fixedTopics`: temas fijos (la bandeja); sin esto, los del shell y la pantalla visible.
   */
  constructor(
    private readonly source: Source,
    private readonly location: { hash: string } | null = null,
    private readonly fixedTopics: Topic[] | null = null,
  ) {
    if (location) this.route = parseHash(location.hash);
  }

  /** La bandeja no tiene asistente: solo la ventana principal se abre al emparejar. */
  private get opensOnPairing(): boolean {
    return this.fixedTopics === null;
  }

  private topics(): Topic[] {
    return this.fixedTopics ?? topicsFor(this.route);
  }

  /** Arranca la fuente. Si falla (agente ausente, respuesta rara) queda el estado «sin agente». */
  async start(): Promise<void> {
    try {
      await this.source.start((d) => {
        const before = this.desk.ready ? this.desk.pair.pairing : null;
        this.desk = d;
        // Si la otra máquina empezó a emparejar (o apareció el código), el asistente se abre en ese paso.
        if (this.opensOnPairing && pairingShouldOpen(before, d.pair.pairing)) this.openWizard(WIZARD_PAIR);
      });
    } catch (e) {
      this.desk = deskFromError(e);
    }
    this.source.watch(this.topics());
    // No se espera: si la fuente tarda en dar los avisos (o no los tiene), la ventana ya se dibuja.
    void this.listenTxn();
  }

  /** Se queda oyendo el avance de las transacciones; sin eso (fuente sin ellas) no hay progreso que mostrar. */
  private async listenTxn(): Promise<void> {
    try {
      this.stopTxn = await this.source.watchTxn((e, at, given) => {
        const before = this.run;
        const origin = e.type === "started" ? (given ?? this.origin) : null;
        // Cada tipo de transacción tiene su tarjeta: el encendido va aparte de la escena.
        if (e.type === "started" && e.steps[0]?.step === "power_on") this.powerRun = feedRun(this.powerRun, e, at ?? this.nowMs(), origin);
        else {
          if (e.type !== "started") this.powerRun = feedRun(this.powerRun, e, at ?? this.nowMs());
          this.run = feedRun(this.run, e, at ?? this.nowMs(), origin);
        }
        if (e.type === "started") this.origin = null;
        this.clock = this.nowMs();
        if ((running(this.run) || running(this.powerRun)) && this.clockTimer === undefined) this.clockTimer = setInterval(() => (this.clock = this.nowMs()), 100);
        if (!running(this.run) && !running(this.powerRun)) this.stopClock();
        // Un cambio que falló y se deshizo se avisa aunque no se esté mirando el progreso.
        if (e.type === "failed" && running(before) && this.route.screen !== "progreso" && this.run) {
          this.toast(this.runView(this.run)?.message.text ?? "No se pudo aplicar el cambio", "bad");
        }
        // Lo que sale bien se ve en la pantalla de Monitores (el selector, la escena activa); en las demás se avisa.
        if (e.type === "committed" && this.run?.doneText && !["monitores", "progreso", "escena"].includes(this.route.screen)) this.toast(this.run.doneText, "ok");
      });
    } catch {
      // Sin avisos de transacciones (fuera de la app): no hay progreso que dibujar.
    }
    this.txnReady = true;
  }

  private nowMs(): number {
    return this.source.now?.() ?? Date.now();
  }

  private stopClock(): void {
    clearInterval(this.clockTimer);
    this.clockTimer = undefined;
    this.clock = this.nowMs();
  }

  /** Los nombres para dibujar el progreso. */
  get names(): Names {
    return { machine: (m) => this.desk.machines[m]?.name ?? machineName(m), monitors: this.desk.monitors };
  }

  /** Lo que dibuja la pantalla de progreso, o `null` si no hay transacción. */
  runView(run: Run | null = this.run): RunView | null {
    return run ? viewOf(run, this.names, this.clock) : null;
  }

  stop(): void {
    this.stopClock();
    this.stopTxn?.();
    this.source.stop();
  }

  /** Cambia de pantalla y se suscribe solo a lo que muestra la nueva. */
  go(route: Route): void {
    this.paletteOpen = false;
    if (sameRoute(route, this.route)) return;
    const before = this.route.screen;
    this.route = route;
    if (this.location && this.location.hash !== formatHash(route)) this.location.hash = formatHash(route);
    if (before !== route.screen && !this.fixedTopics) this.source.watch(this.topics());
  }

  /** La dirección cambió desde fuera (atrás/adelante del historial). */
  syncFromLocation(): void {
    if (this.location) this.go(parseHash(this.location.hash));
  }

  can(what: Capability): boolean {
    return this.source.can(what);
  }

  private async report(p: Promise<ActResult>): Promise<ActResult> {
    let r: ActResult;
    try {
      r = await p;
    } catch (e) {
      r = { ok: false, reason: e instanceof Error ? e.message : String(e) };
    }
    if (!r.ok) this.toast(r.reason, "bad");
    else if (r.text) this.toast(r.text, "ok");
    return r;
  }

  /** Lleva a la pantalla de progreso de la transacción en curso (la de Monitores y el editor lo piden al aplicar). */
  showProgress(): void {
    this.go({ screen: "progreso" });
  }

  /** Aplica una escena; si no se pudo empezar, avisa. Su avance llega como transacción. */
  async applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    this.origin = { title: `Aplicando la escena «${scene?.name ?? id}»`, doneText: `Escena «${scene?.name ?? id}» activa.` };
    const r = await this.report(this.source.applyScene(id));
    if (!r.ok) this.origin = null;
    return r;
  }

  /** Pide la entrada de un monitor compatible. El veredicto lo da el monitor: la pantalla no adivina. */
  async switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === monitor);
    this.origin = { title: `Cambiando la entrada del ${m?.name ?? monitor}`, doneText: "Entrada cambiada." };
    const r = await this.report(this.source.switchMonitor(monitor, input));
    if (!r.ok) this.origin = null;
    return r;
  }

  saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult> {
    return this.report(this.source.saveScene(scene, isNew));
  }

  /** El id que el agente le dio a una escena recién guardada (llega con el puesto nuevo); `null` si no aparece. */
  async waitForScene(name: string, timeoutMs = 3000): Promise<string | null> {
    const until = Date.now() + timeoutMs;
    for (;;) {
      const s = this.desk.scenes.find((x) => x.name === name);
      if (s) return s.id;
      if (Date.now() >= until) return null;
      await new Promise((r) => setTimeout(r, 50));
    }
  }

  forgetMonitor(id: string): Promise<ActResult> {
    return this.report(this.source.forgetMonitor(id));
  }

  setPower(machine: Machine, on: boolean): Promise<ActResult> {
    return this.report(this.source.setPower(machine, on));
  }

  async measureLink(path?: LinkKind): Promise<ActResult> {
    const kind = path ?? this.desk.link.kind;
    const r = await this.report(this.source.measureLink(path));
    if (r.ok && kind) this.measured = kind;
    return r;
  }

  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    return this.report(this.source.setModule(machine, module, on));
  }

  setArrangement(a: Arrangement): Promise<ActResult> {
    return this.report(this.source.setArrangement(a));
  }

  watchCrossing(onCross: (s: CrossSample) => void): Promise<() => void> {
    return this.source.watchCrossing(onCross);
  }

  /** El tiempo para cruzar en la prueba (`undefined`: el de siempre). */
  get crossTimeoutMs(): number | undefined {
    return this.source.crossTimeoutMs;
  }

  rescan(machine: Machine): Promise<ActResult> {
    return this.report(this.source.rescan(machine));
  }

  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    return this.report(this.source.editMachine(machine, edit));
  }

  pairAction(kind: PairActionKind): Promise<ActResult> {
    return this.report(this.source.pairAction(kind));
  }

  requestPermissions(): Promise<ActResult> {
    return this.report(this.source.requestPermissions());
  }

  openPermission(p: Permission): Promise<ActResult> {
    return this.report(this.source.openPermission(p));
  }

  restartAgent(): Promise<ActResult> {
    return this.report(this.source.restartAgent());
  }

  /** En macOS, la primera vez el asistente se abre solo en el paso de permisos (T-090). */
  async maybeFirstRun(): Promise<void> {
    try {
      const v = await this.source.setup();
      if (v.show) this.openWizard(WIZARD_PERMISSIONS);
    } catch {
      // Sin guía (otro sistema o sin la app): nada que abrir.
    }
  }

  async finishSetup(): Promise<void> {
    try {
      await this.source.finishSetup();
    } catch {
      // Si no se pudo guardar, la guía vuelve a abrirse la próxima vez: no es un error visible.
    }
  }

  shortcuts(): Promise<ShortcutsView> {
    return this.source.shortcuts();
  }

  setShortcuts(b: Bindings): Promise<ShortcutsView> {
    return this.source.setShortcuts(b);
  }

  hideMain(): Promise<void> {
    return this.source.hideMain();
  }

  updates(onChange: (v: UpdateView) => void): Promise<UpdateView> {
    return this.source.updates(onChange);
  }

  checkUpdates(): Promise<UpdateView> {
    return this.source.checkUpdates();
  }

  updateAll(): Promise<UpdateView> {
    return this.source.updateAll();
  }

  setAutoUpdate(on: boolean): Promise<UpdateView> {
    return this.source.setAutoUpdate(on);
  }

  /** Paso del asistente con el que se abre (0 = el primero). */
  wizardStep = $state(0);

  openWizard(step = 0): void {
    this.paletteOpen = false;
    this.wizardStep = step;
    this.wizardOpen = true;
  }

  togglePalette(open = !this.paletteOpen): void {
    this.paletteOpen = open;
  }

  toast(text: string, tone: Toast["tone"] = "ok"): void {
    const t = { id: this.nextToast++, text, tone };
    this.toasts = [...this.toasts, t];
    setTimeout(() => this.dismiss(t.id), TOAST_MS);
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }
}
