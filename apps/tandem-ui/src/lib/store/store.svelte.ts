// El estado de la ventana: el puesto (de la fuente real o del demo), la pantalla visible, la
// paleta y los avisos. Las pantallas leen de aquí y piden acciones aquí.

import type { CommandContext } from "../commands";
import { shouldOpen as pairingShouldOpen } from "../pairing";
import type { AdminView, Machine, MonitorInput, SceneJson } from "../model";
import { feedRun, running, viewOf, type Names, type Run, type RunView } from "../txn";
import { machineName } from "../machines";
import type { Bindings, DragStatus, Permission, ShortcutsView } from "../model";
import { dragNotice } from "../drag";
import { HOME, SCREEN_TOPICS, formatHash, parseHash, sameRoute, type Route } from "../routes";
import { EMPTY_DESK, deskFromError } from "./live";
import { changeToast, deskEvents, transferToast, type ToastSpec, type ToastTone } from "../notices";
import type { ActResult, Capability, CursorEvent, DeskView, FilesView, LinkKind, OutputChunk, OutputCursor, PairActionKind, ProcessRow, Source, Topic } from "./types";
import type { UpdateView } from "../updates";
import type { Arrangement } from "../acomodo";
import type { CrossSample } from "../crossing";
import { STEP_PAIR, STEP_PERMISSIONS } from "../wizard";

/** Pasos del asistente que se abren desde otras partes (índices de `WIZARD_STEPS`). */
export const WIZARD_PAIR = STEP_PAIR;
export const WIZARD_PERMISSIONS = STEP_PERMISSIONS;

/** Lo que el shell muestra siempre (barra lateral: máquinas y contador de túneles). */
export const SHELL_TOPICS: Topic[] = ["machines", "tunnels"];

/** La bandeja: todo lo que lista, sin pantallas. */
export const TRAY_TOPICS: Topic[] = ["machines", "link", "scenes", "tunnels", "resources"];

/** Temas del shell más los de la pantalla, sin repetir. */
export function topicsFor(route: Route): Topic[] {
  return [...new Set([...SHELL_TOPICS, ...SCREEN_TOPICS[route.screen]])];
}

export interface Toast extends ToastSpec {
  id: number;
  /** Se va solo a los 6 s; los que piden una decisión se quedan hasta cerrarlos. */
  auto: boolean;
}

/** Lo que dura un aviso en pantalla (`F-Notices`: «se van solos en 6 s»). */
export const TOAST_MS = 6000;
/** Cuántos avisos se apilan a la vez. */
export const TOAST_MAX = 5;

export class Store implements CommandContext {
  desk: DeskView = $state(EMPTY_DESK);
  route: Route = $state(HOME);
  paletteOpen = $state(false);
  wizardOpen = $state(false);
  toasts: Toast[] = $state([]);
  /** La transacción que se está viendo o acaba de terminar (escena, cambio de entrada). */
  run: Run | null = $state(null);
  /** El encendido de una máquina (otra tarjeta de «Changes in progress»). */
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
  private stopDrag: (() => void) | null = null;
  /** El aviso del arrastre in progress: se actualiza en lugar de apilar uno por archivo. */
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
      await this.source.start(
        (d) => {
        const prev = this.desk;
        const before = prev.ready ? prev.pair.pairing : null;
        this.desk = d;
        // Los avisos de lo que cambió, solo en la ventana principal; la bandeja no los repite.
        if (this.opensOnPairing) for (const spec of deskEvents(prev, d)) this.notify(spec);
        // Si the other machine empezó a emparejar (o apareció el código), el asistente se abre en ese paso.
        if (this.opensOnPairing && pairingShouldOpen(before, d.pair.pairing)) this.openWizard(WIZARD_PAIR);
        },
        (c) => {
          if (this.opensOnPairing) this.notify(changeToast(c, this.desk, (rev) => void this.undoDesk(rev)));
        },
        (t) => {
          if (this.opensOnPairing) this.notify(transferToast(t, this.desk));
        },
      );
    } catch (e) {
      this.desk = deskFromError(e);
    }
    this.source.watch(this.topics());
    // No se espera: si la fuente tarda en dar los avisos (o no los tiene), la ventana ya se dibuja.
    void this.listenTxn();
    void this.listenDrag();
  }

  /** El avance del arrastre de archivos va en un solo aviso que se actualiza (T-121). */
  private async listenDrag(): Promise<void> {
    try {
      this.stopDrag = (await this.source.watchDrag?.((s) => this.showDrag(s))) ?? null;
    } catch {
      // Sin avisos del agente (fuera de la app): no hay arrastre que mostrar.
    }
  }

  /** Pinta el estado del arrastre en su aviso. */
  showDrag(s: DragStatus): void {
    const n = dragNotice(s);
    // Con clave, cada estado reemplaza al aviso anterior; mientras sigue in progress no se va solo.
    this.notify({ key: "drag", text: n.text, tone: n.tone, hold: n.ongoing });
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
        if (e.type === "failed" && running(before) && this.sceneAsked && before?.title?.includes(this.sceneAsked.name)) this.failedScene = this.sceneAsked;
        if (e.type === "committed") this.failedScene = null;
        if (e.type === "failed" && running(before) && this.route.screen !== "progreso" && this.run) {
          this.toast(this.runView(this.run)?.message.text ?? "Could not apply the change", "bad");
        }
        // Lo que sale bien se ve en la pantalla de Monitores (el selector, la escena activa); en las demás se avisa.
        if (e.type === "committed" && this.run?.doneText && !["monitores", "progreso", "escena"].includes(this.route.screen)) this.toast(this.run.doneText, "ok");
      });
    } catch {
      // Sin avisos de transacciones (fuera de la app): no hay progreso que dibujar.
    }
    this.txnReady = true;
  }

  /** La hora de la fuente (la del reloj, salvo el demo quieto). */
  now(): number {
    return this.nowMs();
  }

  /** Los procesos de un túnel (pantalla Túneles, solo lectura). */
  processes(tunnel: string): Promise<ProcessRow[]> {
    return this.source.processes(tunnel);
  }

  /** La salida de un proceso desde un cursor. */
  output(tunnel: string, process: number, from: OutputCursor): Promise<OutputChunk> {
    return this.source.output(tunnel, process, from);
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
    this.stopDrag?.();
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
    // `source.can` lee un campo común: leer `desk` (reactivo) hace que lo que lo use se
    // vuelva a evaluar cuando llega una foto nueva, y no se quede con la primera (T-153).
    void this.desk;
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

  /** Lleva a la pantalla de progreso de la transacción in progress (la de Monitores y el editor lo piden al aplicar). */
  showProgress(): void {
    this.go({ screen: "progreso" });
  }

  /** Es el modo demo (la pantalla de Estados muestra entonces su galería). */
  get demo(): boolean {
    return this.source.demo === true;
  }

  /** La escena que se pidió aplicar y la última que falló (D6): «Reintentar» reaplica esa. */
  private sceneAsked: { id: string; name: string } | null = null;
  failedScene: { id: string; name: string } | null = $state(null);

  /** Aplica una escena; si no se pudo empezar, avisa. Su avance llega como transacción. */
  async applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    this.sceneAsked = { id, name: scene?.name ?? id };
    this.origin = { title: `Applying the scene “${scene?.name ?? id}”`, doneText: `Scene “${scene?.name ?? id}” active.` };
    const r = await this.report(this.source.applyScene(id));
    if (!r.ok) this.origin = null;
    return r;
  }

  /** Pide la entrada de un monitor compatible. El veredicto lo da the monitor: la pantalla no adivina. */
  async switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === monitor);
    this.origin = { title: `Changing the input of ${m?.name ?? monitor}`, doneText: "Input changed." };
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

  /** La ventana oyó actividad: el agente la anuncia a the other machine. Sin agente, no pasa nada. */
  reportHeardKey(): Promise<void> {
    return this.source.reportHeardKey?.().catch(() => {}) ?? Promise.resolve();
  }

  watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void> {
    return this.source.watchKeyProbe(onHeard);
  }

  watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void> {
    return this.source.watchCursor(onEvent);
  }

  /** El tiempo para cruzar en la prueba (`undefined`: el de siempre). */
  get crossTimeoutMs(): number | undefined {
    return this.source.crossTimeoutMs;
  }

  rescan(machine: Machine): Promise<ActResult> {
    return this.report(this.source.rescan(machine));
  }

  setDeskName(name: string): Promise<ActResult> {
    return this.report(this.source.setDeskName(name));
  }

  setWol(mac: string | null): Promise<ActResult> {
    return this.report(this.source.setWol(mac));
  }

  editMachine(machine: Machine, edit: { name?: string; color?: string }): Promise<ActResult> {
    return this.report(this.source.editMachine(machine, edit));
  }

  pairAction(kind: PairActionKind): Promise<ActResult> {
    return this.report(this.source.pairAction(kind));
  }

  lockHere(on: boolean): Promise<ActResult> {
    return this.report(this.source.lockHere(on));
  }

  requestPermissions(): Promise<ActResult> {
    return this.report(this.source.requestPermissions());
  }

  openPermission(p: Permission): Promise<ActResult> {
    return this.report(this.source.openPermission(p));
  }

  /** Lo que reporta el servicio de administrador; `null` hasta la primera lectura. */
  admin = $state<AdminView | null>(null);
  /** «Instalar» in progress (diálogo de administrador abierto). */
  adminBusy = $state(false);

  async refreshAdmin(): Promise<void> {
    try {
      this.admin = await this.source.adminStatus();
    } catch {
      this.admin = { state: "off", canInstall: false, note: "could not read the service state" };
    }
  }

  /** Instala el servicio. Cancelar o fallar deja la fila en su estado real y avisa, sin giro eterno. */
  async installAdmin(): Promise<ActResult> {
    if (this.adminBusy) return { ok: false, reason: "it is already being installed" };
    this.adminBusy = true;
    try {
      const r = await this.report(this.source.installAdmin().then((v): ActResult => ((this.admin = v), { ok: true, text: "Administrator service installed" })));
      if (!r.ok) await this.refreshAdmin();
      return r;
    } finally {
      this.adminBusy = false;
    }
  }

  restartAgent(): Promise<ActResult> {
    return this.report(this.source.restartAgent());
  }

  /**
   * La primera vez el asistente se abre solo, en cualquier sistema: en el de permisos si hay
   * permisos que conceder (T-090) y, si no, en «Your desk» mientras no haya par (T-156).
   */
  async maybeFirstRun(): Promise<void> {
    try {
      const v = await this.source.setup();
      if (!v.show) return;
      if (v.available) this.openWizard(WIZARD_PERMISSIONS);
      else if (this.desk.pair.peer.kind !== "paired") this.openWizard(WIZARD_PAIR);
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

  /** El cruce del paso 5 pasó con el acomodo guardado (lo usa el resumen). */
  wizardCrossed = $state(false);

  openWizard(step = 0): void {
    this.paletteOpen = false;
    this.wizardCrossed = false;
    this.wizardStep = step;
    this.wizardOpen = true;
  }

  togglePalette(open = !this.paletteOpen): void {
    this.paletteOpen = open;
  }

  toast(text: string, tone: ToastTone = "ok"): void {
    this.notify({ text, tone });
  }

  /** Un aviso nuevo arriba de la pila; el más viejo sale si pasan de `TOAST_MAX`. */
  notify(spec: ToastSpec): void {
    const auto = !spec.action && !spec.progress && !spec.hold;
    const at = spec.key ? this.toasts.findIndex((x) => x.key === spec.key) : -1;
    // El avance de una copia actualiza su aviso en el sitio; no apila uno por cada muestra.
    const t: Toast = { ...spec, id: at >= 0 ? this.toasts[at].id : this.nextToast++, auto };
    this.toasts = at >= 0 ? this.toasts.map((x, i) => (i === at ? t : x)) : [t, ...this.toasts].slice(0, TOAST_MAX);
    if (t.auto) setTimeout(() => this.dismiss(t.id), TOAST_MS);
  }

  /** «Deshacer» del aviso de cambio: vuelve el puesto a la versión que había. */
  undoDesk(rev: number): Promise<ActResult> {
    return this.report(this.source.undoDesk(rev));
  }

  setPrefs(p: { cmdCtrl: boolean; dragFiles: boolean }): Promise<ActResult> {
    return this.report(this.source.setPrefs(p));
  }

  setLockShortcut(lock: string[]): Promise<ActResult> {
    return this.report(this.source.setLockShortcut(lock));
  }

  files(): Promise<FilesView> {
    return this.source.files();
  }

  openFiles(target: "config" | "logs"): Promise<ActResult> {
    return this.report(this.source.openFiles(target));
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }
}
