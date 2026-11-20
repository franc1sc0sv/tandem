// App notices (F-Notices): which desk change deserves a notice and with what text. Pure:
// the store calls it with the previous state and the current one.

import type { Machine } from "./model";
import { shortMonitor } from "./monitors";
import type { DeskChange, DeskView, LinkKind, MonitorUi, TransferView } from "./store/types";

export type ToastTone = "ok" | "bad" | "warn" | "info" | "claude";

/** Dot color of each tone (`F-Notices`). */
export const TONE_COLOR: Record<ToastTone, string> = { ok: "#6cc59a", bad: "#e07a6f", warn: "#d9a75a", info: "#7aa7e6", claude: "#a99be8" };

export interface ToastAction {
  label: string;
  run: () => void;
}

/** What a notice asks for; the store gives it the id and turns it off after 6 s if it does not ask for a decision. */
export interface ToastSpec {
  /** With a key, a new notice replaces the one that already has it (the progress of a copy). */
  key?: string;
  /** Progress with a bar: the notice does not go away by itself until it finishes. */
  progress?: { done: number; total: number };
  text: string;
  body?: string;
  tone: ToastTone;
  action?: ToastAction;
  /** Does not go away by itself (the `F-Notices` demo, to capture the whole stack). */
  hold?: boolean;
}

export const KIND_NAME: Record<LinkKind, string> = { usb4: "USB-C", ethernet: "Ethernet", wifi: "Wi-Fi" };

const nameOf = (d: DeskView, m: Machine) => d.machines[m]?.name ?? `machine ${m.toUpperCase()}`;

/** Where a tunnel's processes run on `m`, as the notice names it: "WSL", "zsh", "Docker". */
function envLabel(d: DeskView, m: Machine): string | null {
  const envs = d.machines[m]?.environments ?? [];
  const env = envs.find((e) => e.kind === "wsl" && e.default) ?? envs.find((e) => e.kind === "shell" && e.default) ?? envs[0];
  if (!env) return null;
  return env.kind === "wsl" ? "WSL" : env.kind === "docker" ? "Docker" : env.name;
}

/**
 * Sample notices of demo mode (`?demo&notices`): the three `F-Notices` opens with, from newest
 * to oldest. They are seeded only on request, to see and capture the stack. The names come from
 * the desk (each pair has its own machines and environments); with the mockup's pair they read
 * exactly as the mockup. The "Send over Wi-Fi anyway" button does nothing: the agent has no such function and the real app does not show it; it is here so the stack measures the same as the mockup.
 */
export function demoNotices(d: DeskView): ToastSpec[] {
  const scene = d.scenes.find((s) => s.id === "work") ?? d.scenes[0];
  const first = scene?.monitors[0];
  const monitor = d.monitors.find((m) => m.id === first?.monitor)?.name;
  // "Odyssey G5 34\"" is shown as `34"`, like the mockup; a name without a size is kept whole.
  const size = monitor?.match(/\d+(?:\.\d+)?"/)?.[0] ?? monitor ?? "The monitor";
  const shown = first ? nameOf(d, first.machine) : nameOf(d, d.here === "a" ? "b" : "a");
  const peer: Machine = d.here === "a" ? "b" : "a";
  const env = envLabel(d, peer);
  return [
    { text: "The USB-C cable was unplugged", body: "Keyboard and mouse carry on over Wi-Fi. Data transfers wait for the cable.", tone: "warn", action: { label: "Send over Wi-Fi anyway", run: () => {} } },
    { text: `Scene “${scene?.name ?? "Work"}” applied`, body: `${size} confirmed on ${shown} in 1.8 s.`, tone: "ok" },
    { text: "Claude opened the tunnel “tandem”", body: `${nameOf(d, d.here)} → ${nameOf(d, peer)}${env ? ` · ${env}` : ""}.`, tone: "claude" },
  ];
}

/** Readable bytes: "1.4 GB", "320 MB", "12 KB". */
export function bytesText(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(1)} GB`;
  if (n >= 1e6) return `${Math.round(n / 1e6)} MB`;
  return `${Math.max(0, Math.round(n / 1e3))} KB`;
}

/** The notice of a file copy: with a bar while it advances and "Done" when it ends. */
export function transferToast(t: TransferView, d: DeskView): ToastSpec {
  const name = t.path.split(/[\\/]/).filter(Boolean).pop() ?? t.path;
  const peer = nameOf(d, d.here === "a" ? "b" : "a");
  const key = `transfer:${t.path}`;
  if (t.finished) return { key, text: `Copied “${name}”`, body: t.upload ? `It is now on ${peer}.` : `It is now on this machine.`, tone: "ok" };
  const pct = t.total > 0 ? Math.min(100, Math.floor((t.done / t.total) * 100)) : 0;
  return {
    key,
    text: t.upload ? `Sending “${name}” to ${peer}` : `Receiving “${name}” from ${peer}`,
    body: `${pct} % · ${bytesText(t.done)} of ${bytesText(t.total)}`,
    tone: "info",
    progress: { done: t.done, total: t.total },
  };
}

/** "Changed from X · Undo" (D5): the notice waits for the decision, it does not go away by itself. */
export function changeToast(c: DeskChange, d: DeskView, undo: (rev: number) => void): ToastSpec {
  return {
    text: `Changed from ${nameOf(d, c.by)}`,
    body: c.summary,
    tone: "info",
    action: { label: "Undo", run: () => undo(c.undoTo) },
  };
}

/** The notices that result from going from `before` to `after` (path change, tunnel, machine). */
export function deskEvents(before: DeskView, after: DeskView): ToastSpec[] {
  if (!before.ready || !after.ready) return [];
  const out: ToastSpec[] = [];

  const was = before.link.kind;
  const now = after.link.kind;
  if (was && now && was !== now) {
    if (now === "wifi") {
      out.push({ text: `The ${KIND_NAME[was]} cable was unplugged`, body: "Keyboard and mouse carry on over Wi-Fi. Data transfers wait for the cable.", tone: "warn" });
    } else {
      out.push({ text: `The ${KIND_NAME[now]} cable is back`, body: "Input and transfers go over the cable again.", tone: "ok" });
    }
  }

  const known = new Set(before.tunnels.map((t) => t.id));
  for (const t of after.tunnels) {
    if (known.has(t.id) || t.state === "closed") continue;
    out.push({ text: `Claude opened the tunnel “${t.title}”`, body: `${nameOf(after, t.from)} → ${nameOf(after, t.to)}`, tone: "claude" });
  }

  const peer: Machine = after.here === "a" ? "b" : "a";
  const p0 = before.machines[peer]?.power;
  const p1 = after.machines[peer]?.power;
  if (p0 && p1 && p0 !== p1) {
    if (p1 === "on") out.push({ text: `${nameOf(after, peer)} is back`, body: "Powered on and back on the desk.", tone: "claude" });
    else if (p0 === "on" && (p1 === "off" || p1 === "lost")) {
      out.push({ text: `${nameOf(after, peer)} ${p1 === "off" ? "went off" : "was lost"}`, body: "The mouse no longer crosses to it.", tone: "warn" });
    }
  }
  return out;
}

/**
 * What was last said about each monitor (T-349): each cable end and whether it was off. Only
 * known values are kept, so an `unknown` in between never makes a notice come back.
 */
export type LinkMemo = Record<string, { a?: "connected" | "disconnected"; b?: "connected" | "disconnected"; off?: boolean }>;

/**
 * The notices of the monitors' link transitions: "Mac cable unplugged", "Mac cable connected",
 * "34" turned off", "34" back on". The first state seen of a monitor is remembered without a
 * notice, and the same state is never announced twice in a row.
 */
export function monitorNotices(memo: LinkMemo, monitors: MonitorUi[], name: (m: Machine) => string): { specs: ToastSpec[]; memo: LinkMemo } {
  const next: LinkMemo = { ...memo };
  const specs: ToastSpec[] = [];
  for (const m of monitors) {
    if (!m.link) continue;
    const was = memo[m.id];
    const now = { ...(was ?? {}) };
    const size = shortMonitor(m.name);
    for (const who of ["a", "b"] as Machine[]) {
      const c = m.link.cables[who];
      if (c === "unknown") continue;
      if (was?.[who] !== undefined && was[who] !== c) {
        specs.push(
          c === "disconnected"
            ? { text: `${name(who)} cable unplugged`, body: `${size} has no picture from ${name(who)} until it is plugged back in.`, tone: "warn" }
            : { text: `${name(who)} cable connected`, body: `${size} sees ${name(who)} again.`, tone: "ok" },
        );
      }
      now[who] = c;
    }
    if (m.link.kind !== "unknown") {
      const off = m.link.kind === "off";
      if (was?.off !== undefined && was.off !== off) {
        specs.push(off ? { text: `${size} turned off`, body: "The cursor stays on the screens that are on.", tone: "warn" } : { text: `${size} back on`, tone: "ok" });
      }
      now.off = off;
    }
    next[m.id] = now;
  }
  return { specs, memo: next };
}
