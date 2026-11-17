// El estado de la ventana: el puesto (de la fuente real o del demo), la pantalla visible, la
// paleta y los avisos. Las pantallas leen de aquí y piden acciones aquí.

import type { CommandContext } from "../commands";
import type { Machine } from "../model";
import { HOME, SCREEN_TOPICS, formatHash, parseHash, sameRoute, type Route } from "../routes";
import { EMPTY_DESK } from "./live";
import type { ActResult, DeskView, Source } from "./types";

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
  /** Número de túneles activos, para la barra lateral. */
  activeTunnels = $derived(this.desk.tunnels.filter((t) => t.state === "busy").length);

  private nextToast = 1;

  constructor(
    private readonly source: Source,
    private readonly location: { hash: string } | null = null,
  ) {
    if (location) this.route = parseHash(location.hash);
  }

  async start(): Promise<void> {
    await this.source.start((d) => {
      this.desk = d;
    });
    this.source.watch(SCREEN_TOPICS[this.route.screen]);
  }

  stop(): void {
    this.source.stop();
  }

  /** Cambia de pantalla y se suscribe solo a lo que muestra la nueva. */
  go(route: Route): void {
    this.paletteOpen = false;
    if (sameRoute(route, this.route)) return;
    const before = this.route.screen;
    this.route = route;
    if (this.location && this.location.hash !== formatHash(route)) this.location.hash = formatHash(route);
    if (before !== route.screen) this.source.watch(SCREEN_TOPICS[route.screen]);
  }

  /** La dirección cambió desde fuera (atrás/adelante del historial). */
  syncFromLocation(): void {
    if (this.location) this.go(parseHash(this.location.hash));
  }

  can(what: "scenes" | "modules" | "power" | "measure"): boolean {
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

  applyScene(id: string): Promise<ActResult> {
    return this.report(this.source.applyScene(id));
  }

  setPower(machine: Machine, on: boolean): Promise<ActResult> {
    return this.report(this.source.setPower(machine, on));
  }

  measureLink(): Promise<ActResult> {
    return this.report(this.source.measureLink());
  }

  setModule(machine: Machine, module: string, on: boolean): Promise<ActResult> {
    return this.report(this.source.setModule(machine, module, on));
  }

  openWizard(): void {
    this.paletteOpen = false;
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
