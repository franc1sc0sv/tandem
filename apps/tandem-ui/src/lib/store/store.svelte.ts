// The window state: the desk (from the real source or the demo), the visible screen, the
// palette and the notices. The screens read from here and ask for actions here.

import type { CommandContext } from "../commands";
import { shouldOpen as pairingShouldOpen } from "../pairing";
import type { AdminView, Machine, MonitorInput, SceneJson } from "../model";
import { feedRun, running, viewOf, type Names, type Run, type RunView } from "../txn";
import { machineName } from "../machines";
import type { Bindings, DragStatus, Permission, ShortcutsView } from "../model";
import { dragNotice } from "../drag";
import { HOME, SCREEN_TOPICS, formatHash, parseHash, sameRoute, type Route } from "../routes";
import { EMPTY_DESK, deskFromError } from "./live";
import { changeToast, demoNotices, deskEvents, transferToast, type ToastSpec, type ToastTone } from "../notices";
import type { ActResult, Capability, CursorEvent, DeskView, FilesView, LinkKind, OutputChunk, OutputCursor, PairActionKind, ProcessRow, Source, Topic } from "./types";
import type { UpdateView } from "../updates";
import type { Arrangement } from "../arrangement";
import type { CrossSample } from "../crossing";
import { STEP_PAIR, STEP_PERMISSIONS } from "../wizard";

/** Setup steps opened from other places (indices of `WIZARD_STEPS`). */
export const WIZARD_PAIR = STEP_PAIR;
export const WIZARD_PERMISSIONS = STEP_PERMISSIONS;

/** What the shell always shows (sidebar: machines and tunnel counter). */
export const SHELL_TOPICS: Topic[] = ["machines", "tunnels"];

/** The tray: everything it lists, without screens. */
export const TRAY_TOPICS: Topic[] = ["machines", "link", "scenes", "tunnels", "resources"];

/** Shell topics plus those of the screen, without repeats. */
export function topicsFor(route: Route): Topic[] {
  return [...new Set([...SHELL_TOPICS, ...SCREEN_TOPICS[route.screen]])];
}

export interface Toast extends ToastSpec {
  id: number;
  /** Goes away by itself after 6 s; those that ask for a decision stay until closed. */
  auto: boolean;
}

/** How long a notice stays on screen (`F-Notices`: "they go away by themselves in 6 s"). */
export const TOAST_MS = 6000;
/** How many notices stack at once. */
export const TOAST_MAX = 5;
/** How long the window waits for the first notice of a transaction it asked for (T-250). */
export const EXPECT_TXN_MS = 4000;

type Origin = { title: string; doneText: string };

export class Store implements CommandContext {
  desk: DeskView = $state(EMPTY_DESK);
  route: Route = $state(HOME);
  paletteOpen = $state(false);
  wizardOpen = $state(false);
  toasts: Toast[] = $state([]);
  /** The transaction being watched or just finished (scene, input change). */
  run: Run | null = $state(null);
  /** A machine powering on (another "Changes in progress" card). */
  powerRun: Run | null = $state(null);
  /** The last link measured from this window (its card stays visible). */
  measured: LinkKind | null = $state(null);
  /** Clock of the progress screen (counts the time while it runs). */
  clock = $state(Date.now());
  /** It is already known whether there are transaction notices: before that, "no progress" does not mean anything yet. */
  txnReady = $state(false);
  /**
   * The transaction this window asked for whose `started` has not arrived yet (T-250). The
   * agent's reply and the transaction's notices travel on different channels, so the reply can
   * win: until `started` arrives the progress screen waits instead of leaving.
   */
  expectedTxn: number | null = $state(null);
  /** Number of active tunnels, for the sidebar. */
  activeTunnels = $derived(this.desk.tunnels.filter((t) => t.state === "busy").length);

  private nextToast = 1;
  /** What was requested from this window: gives the title and final text of the transaction that starts. */
  private origin: Origin | null = null;
  /** The origin of `expectedTxn`: it is given to that transaction even if another one starts first. */
  private expectedOrigin: Origin | null = null;
  private expectTimer: ReturnType<typeof setTimeout> | undefined;
  private stopTxn: (() => void) | null = null;
  private stopDrag: (() => void) | null = null;
  /** The notice of the drag in progress: it is updated instead of stacking one per file. */
  private clockTimer: ReturnType<typeof setInterval> | undefined;

  /**
   * `fixedTopics`: fixed topics (the tray); without this, those of the shell and the visible screen.
   */
  constructor(
    private readonly source: Source,
    private readonly location: { hash: string } | null = null,
    private readonly fixedTopics: Topic[] | null = null,
  ) {
    if (location) this.route = parseHash(location.hash);
  }

  /** The tray has no setup: only the main window opens it on pairing. */
  private get opensOnPairing(): boolean {
    return this.fixedTopics === null;
  }

  private topics(): Topic[] {
    return this.fixedTopics ?? topicsFor(this.route);
  }

  /** Starts the source. If it fails (agent absent, odd response) the "no agent" state remains. */
  async start(): Promise<void> {
    try {
      await this.source.start(
        (d) => {
        const prev = this.desk;
        const before = prev.ready ? prev.pair.pairing : null;
        this.desk = d;
        // The notices for what changed, only in the main window; the tray does not repeat them.
        if (this.opensOnPairing) for (const spec of deskEvents(prev, d)) this.notify(spec);
        // If the other machine started pairing (or the code appeared), the setup opens at that step.
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
    // It is not awaited: if the source is slow to give the notices (or has none), the window is already drawn.
    void this.listenTxn();
    void this.listenDrag();
  }

  /** The file drag progress goes in a single notice that is updated (T-121). */
  private async listenDrag(): Promise<void> {
    try {
      this.stopDrag = (await this.source.watchDrag?.((s) => this.showDrag(s))) ?? null;
    } catch {
      // Without agent notices (outside the app): there is no drag to show.
    }
  }

  /** Paints the drag state in its notice. */
  showDrag(s: DragStatus): void {
    const n = dragNotice(s);
    // With a key, each state replaces the previous notice; while it is still in progress it does not go away by itself.
    this.notify({ key: "drag", text: n.text, tone: n.tone, hold: n.ongoing });
  }

  /** Keeps listening to the transactions' progress; without that (a source without them) there is no progress to show. */
  private async listenTxn(): Promise<void> {
    try {
      this.stopTxn = await this.source.watchTxn((e, at, given) => {
        const before = this.run;
        const mine = e.type === "started" && e.txn === this.expectedTxn;
        const origin = e.type === "started" ? (given ?? (mine ? this.expectedOrigin : this.origin)) : null;
        if (mine) this.stopExpecting();
        // Each kind of transaction has its card: powering on is separate from the scene.
        if (e.type === "started" && e.steps[0]?.step === "power_on") this.powerRun = feedRun(this.powerRun, e, at ?? this.nowMs(), origin);
        else {
          if (e.type !== "started") this.powerRun = feedRun(this.powerRun, e, at ?? this.nowMs());
          this.run = feedRun(this.run, e, at ?? this.nowMs(), origin);
        }
        if (e.type === "started") this.origin = null;
        this.clock = this.nowMs();
        if ((running(this.run) || running(this.powerRun)) && this.clockTimer === undefined) this.clockTimer = setInterval(() => (this.clock = this.nowMs()), 100);
        if (!running(this.run) && !running(this.powerRun)) this.stopClock();
        // A change that failed and was undone is announced even if the progress is not being viewed.
        if (e.type === "failed" && running(before) && this.sceneAsked && before?.title?.includes(this.sceneAsked.name)) this.failedScene = this.sceneAsked;
        if (e.type === "committed") this.failedScene = null;
        if (e.type === "failed" && running(before) && this.route.screen !== "progress" && this.run) {
          this.toast(this.runView(this.run)?.message.text ?? "Could not apply the change", "bad");
        }
        // What succeeds is seen on the Monitors screen (the selector, the active scene); on the others it is announced.
        if (e.type === "committed" && this.run?.doneText && !["monitors", "progress", "scene"].includes(this.route.screen)) this.toast(this.run.doneText, "ok");
      });
    } catch {
      // Without transaction notices (outside the app): there is no progress to draw.
    }
    this.txnReady = true;
  }

  /** The source time (the clock's, except for the still demo). */
  now(): number {
    return this.nowMs();
  }

  /** The processes of a tunnel (Tunnels screen, read-only). */
  processes(tunnel: string): Promise<ProcessRow[]> {
    return this.source.processes(tunnel);
  }

  /** The output of a process from a cursor. */
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

  /** The names to draw the progress. */
  get names(): Names {
    return { machine: (m) => this.desk.machines[m]?.name ?? machineName(m), monitors: this.desk.monitors };
  }

  /** What the progress screen draws, or `null` if there is no transaction. */
  runView(run: Run | null = this.run): RunView | null {
    return run ? viewOf(run, this.names, this.clock) : null;
  }

  stop(): void {
    this.stopClock();
    this.stopExpecting();
    this.stopTxn?.();
    this.stopDrag?.();
    this.source.stop();
  }

  /** Changes screen and subscribes only to what the new one shows. */
  go(route: Route): void {
    this.paletteOpen = false;
    if (sameRoute(route, this.route)) return;
    const before = this.route.screen;
    this.route = route;
    if (this.location && this.location.hash !== formatHash(route)) this.location.hash = formatHash(route);
    if (before !== route.screen && !this.fixedTopics) this.source.watch(this.topics());
  }

  /** The address changed from outside (back/forward in the history). */
  syncFromLocation(): void {
    if (this.location) this.go(parseHash(this.location.hash));
  }

  can(what: Capability): boolean {
    // `source.can` reads a common field: reading `desk` (reactive) makes whatever uses it
    // re-evaluate when a new snapshot arrives, instead of staying with the first one (T-153).
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

  /** Leads to the progress screen of the transaction in progress (the Monitors one and the editor ask for it when applying). */
  showProgress(): void {
    this.go({ screen: "progress" });
  }

  /** It is demo mode (the States screen then shows its gallery). */
  get demo(): boolean {
    return this.source.demo === true;
  }

  /** The scene requested to apply and the last one that failed (D6): "Retry" reapplies that one. */
  private sceneAsked: { id: string; name: string } | null = null;
  failedScene: { id: string; name: string } | null = $state(null);

  /** Applies a scene; if it could not start, it announces so. Its progress arrives as a transaction. */
  applyScene(id: string): Promise<ActResult> {
    const scene = this.desk.scenes.find((s) => s.id === id);
    this.sceneAsked = { id, name: scene?.name ?? id };
    const name = scene?.name ?? id;
    return this.change({ title: `Applying the scene “${name}”`, doneText: `Scene “${name}” active.` }, () => this.source.applyScene(id));
  }

  /** Requests the input of a compatible monitor. The verdict comes from the monitor: the screen does not guess. */
  switchMonitor(monitor: string, input: MonitorInput): Promise<ActResult> {
    const m = this.desk.monitors.find((x) => x.id === monitor);
    return this.change({ title: `Changing the input of ${m?.name ?? monitor}`, doneText: "Input changed." }, () => this.source.switchMonitor(monitor, input));
  }

  /**
   * A change that runs as a transaction. If the reply names the transaction and its `started`
   * has not arrived yet, the window expects it (T-250): the reply is not proof that the progress
   * is already known.
   */
  private async change(origin: Origin, ask: () => Promise<ActResult>): Promise<ActResult> {
    this.origin = origin;
    const askedAt = this.nowMs();
    const r = await this.report(ask());
    if (!r.ok) this.origin = null;
    else if (r.txn !== undefined && this.stopTxn !== null && !this.startedSince(r.txn, askedAt)) this.expect(r.txn, origin);
    return r;
  }

  /** The `started` of `txn` already arrived (after the request: an agent restart reuses ids). */
  private startedSince(txn: number, at: number): boolean {
    return [this.run, this.powerRun].some((r) => r !== null && r.progress.txn === txn && r.startedAt >= at);
  }

  private expect(txn: number, origin: Origin): void {
    this.stopExpecting();
    this.expectedTxn = txn;
    this.expectedOrigin = origin;
    this.expectTimer = setTimeout(() => {
      if (this.expectedTxn !== txn) return;
      this.stopExpecting();
      this.origin = null;
      this.toast("The change was accepted, but its progress did not arrive", "bad");
      if (this.route.screen === "progress") this.go({ screen: "monitors" });
    }, EXPECT_TXN_MS);
  }

  private stopExpecting(): void {
    clearTimeout(this.expectTimer);
    this.expectTimer = undefined;
    this.expectedTxn = null;
    this.expectedOrigin = null;
  }

  saveScene(scene: SceneJson, isNew: boolean): Promise<ActResult> {
    return this.report(this.source.saveScene(scene, isNew));
  }

  /** The id the agent gave to a newly saved scene (arrives with the new desk); `null` if it does not appear. */
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

  /** The window heard activity: the agent announces it to the other machine. Without an agent, nothing happens. */
  reportHeardKey(): Promise<void> {
    return this.source.reportHeardKey?.().catch(() => {}) ?? Promise.resolve();
  }

  watchKeyProbe(onHeard: (m: Machine) => void): Promise<() => void> {
    return this.source.watchKeyProbe(onHeard);
  }

  watchCursor(onEvent: (e: CursorEvent) => void): Promise<() => void> {
    return this.source.watchCursor(onEvent);
  }

  /** The time to cross in the test (`undefined`: the usual one). */
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

  /** What the administrator service reports; `null` until the first reading. */
  admin = $state<AdminView | null>(null);
  /** "Install" in progress (administrator dialog open). */
  adminBusy = $state(false);

  async refreshAdmin(): Promise<void> {
    try {
      this.admin = await this.source.adminStatus();
    } catch {
      this.admin = { state: "off", canInstall: false, note: "could not read the service state" };
    }
  }

  /** Installs the service. Canceling or failing leaves the row in its real state and announces it, without an endless spinner. */
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
   * The first time the setup opens by itself, on any system: on the permissions one if there are
   * permissions to grant (T-090) and, if not, on "Your desk" while there is no peer (T-156).
   */
  async maybeFirstRun(): Promise<void> {
    try {
      const v = await this.source.setup();
      if (!v.show) return;
      if (v.available) this.openWizard(WIZARD_PERMISSIONS);
      else if (this.desk.pair.peer.kind !== "paired") this.openWizard(WIZARD_PAIR);
    } catch {
      // Without a guide (another system or without the app): nothing to open.
    }
  }

  async finishSetup(): Promise<void> {
    try {
      await this.source.finishSetup();
    } catch {
      // If it could not be saved, the guide opens again next time: it is not a visible error.
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

  /** The newest Updates view seen, from an event or from a command reply (T-251). */
  private updateView: UpdateView | null = null;

  /** Keeps the newest view by its sequence: a late reply does not undo the steps that came by event. */
  private newestUpdate(v: UpdateView): UpdateView {
    if (this.updateView !== null && v.seq < this.updateView.seq) return this.updateView;
    this.updateView = v;
    return v;
  }

  async updates(onChange: (v: UpdateView) => void): Promise<UpdateView> {
    const v = await this.source.updates((e) => {
      if (this.newestUpdate(e) === e) onChange(e);
    });
    return this.newestUpdate(v);
  }

  async checkUpdates(): Promise<UpdateView> {
    return this.newestUpdate(await this.source.checkUpdates());
  }

  async updateAll(): Promise<UpdateView> {
    return this.newestUpdate(await this.source.updateAll());
  }

  async setAutoUpdate(on: boolean): Promise<UpdateView> {
    return this.newestUpdate(await this.source.setAutoUpdate(on));
  }

  /** Step of the setup it opens with (0 = the first). */
  wizardStep = $state(0);

  /** The step 5 crossing passed with the arrangement saved (the summary uses it). */
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

  /** A new notice on top of the stack; the oldest leaves if they exceed `TOAST_MAX`. */
  notify(spec: ToastSpec): void {
    const auto = !spec.action && !spec.progress && !spec.hold;
    const at = spec.key ? this.toasts.findIndex((x) => x.key === spec.key) : -1;
    // The progress of a copy updates its notice in place; it does not stack one per sample.
    const t: Toast = { ...spec, id: at >= 0 ? this.toasts[at].id : this.nextToast++, auto };
    this.toasts = at >= 0 ? this.toasts.map((x, i) => (i === at ? t : x)) : [t, ...this.toasts].slice(0, TOAST_MAX);
    if (t.auto) setTimeout(() => this.dismiss(t.id), TOAST_MS);
  }

  /** Demo (`?notices`): the sample stack, oldest first so the newest ends up on top. */
  seedDemoNotices(): void {
    for (const spec of [...demoNotices(this.desk)].reverse()) this.notify({ ...spec, hold: true });
  }

  /** "Undo" of the change notice: returns the desk to the version it had. */
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
