// The trouble states (`V3-States`): each says what happened, what Tandem did on its own and what
// the user can do. Pure: builds the cards with the real names of the desk.

import type { IconName } from "../ui/icons";
import type { DeskView } from "./store/types";

/** Color of the card icon: background and stroke (`V3-States`). */
export type StateTone = "blue" | "grey" | "amber" | "red";

export const TONE_STYLE: Record<StateTone, { bg: string; fg: string }> = {
  blue: { bg: "#16202e", fg: "#7aa7e6" },
  grey: { bg: "#1b1c21", fg: "#9a9da6" },
  amber: { bg: "#1e1a12", fg: "#d9a75a" },
  red: { bg: "#25171a", fg: "#e07a6f" },
};

/** What a card's button does; the screen wires it to the store. */
export type StateDo = "pair-peer" | "power-on" | "retry-scene" | "tunnels" | "permission";

export interface StateAction {
  label: string;
  icon: IconName;
  variant: "pri" | "default" | "ghost";
  does: StateDo;
  /** The scene that "Retry" reapplies: the one that failed. */
  sceneId?: string;
  /** How long it takes, next to the button ("≈ 40 s"). */
  hint?: string;
}

export interface StateCardSpec {
  id: string;
  tag: string;
  tone: StateTone;
  icon: IconName;
  /** A stroke thicker than most (the action icons). */
  stroke?: number;
  title: string;
  body: string;
  /** A line of job output, in monospace. */
  mono?: string;
  /** Monospace text inside the body, where it says `{code}`. */
  code?: string;
  action?: StateAction;
}

/** How long the other machine takes to wake up (DESIGN §8). */
export const WAKE_HINT = "≈ 40 s";

/**
 * The binary path without the text around it ("Grant the permission to this binary: <path> (pid
 * 12)") and shortened: `/Applications/Tandem.app/Contents/MacOS/tandemd` → `/Applications/Tandem.app/…/tandemd`.
 */
export function shortPath(text: string | null): string {
  const raw = (text ?? "/Applications/Tandem.app/Contents/MacOS/tandemd").replace(/^.*?:\s*/, "").replace(/\s*\(pid \d+\)$/, "");
  const parts = raw.split("/");
  return parts.length > 4 ? [...parts.slice(0, 3), "…", parts[parts.length - 1]].join("/") : raw;
}

/** Where the cards come from: the real state of the desk, or the full demo gallery. */
export interface StateContext {
  /** Demo: all nine are shown, with generic data. In the real app only those that apply. */
  gallery: boolean;
  /** The last scene that could not be applied (the store keeps it when it sees the failure). */
  failedScene: { id: string; name: string } | null;
  /** `?demo&states`: the sample texts of `V3-States` (names, figures), to compare them with the mockup. */
  example?: boolean;
}

/** The mockup's sample texts, per card: the only thing that differs from the generic gallery. */
const EXAMPLE: Record<string, Partial<Pick<StateCardSpec, "title" | "body" | "mono" | "code">>> = {
  off: { title: "PC-Desktop is off", body: "Its card is dimmed and the mouse no longer crosses to it. MacBook Pro can wake it over the network." },
  cable: {
    title: "The USB-C cable was unplugged",
    body: "Keyboard and mouse carried on over Wi-Fi without a break (0.6 s). Sending 2 workspaces is paused until a cable comes back, or you can send them over Wi-Fi now.",
  },
  resumed: { body: "“Train LoRA” kept running on PC-Desktop while there was no link. 3 min of output were recovered.", mono: "epoch 9/20 · loss 0.0371" },
  failed: { title: "“ComfyUI render” failed", body: "Out of GPU memory: “Train LoRA” uses 14.1 of 16 GB on PC-Desktop. Claude sees this reason over MCP and can wait or pick another machine." },
  scene: { title: "The scene “MacBook only” was not applied", body: "The 34\" answered “change failed” (no HDMI signal from the MacBook). Tandem rolled back the scene: everything stayed as it was." },
  unsupported: { title: "Unsupported monitor" },
  nobody: { title: "Nobody can talk to the 34\"", body: "Its owner (PC-Desktop) is off and MacBook Pro reaches it only over HDMI while it is showing it. When either can, it takes control on its own." },
  permission: { title: "“Input Monitoring” is missing on macOS", body: "Without this permission, MacBook Pro cannot send its keyboard. Grant it to {code}; Tandem detects it on its own.", code: "/Applications/Tandem.app/…/tandemd" },
};

/** The cards that apply (or all nine in the gallery), in the mockup's order. */
export function stateCards(d: DeskView, ctx: StateContext = { gallery: false, failedScene: null }): StateCardSpec[] {
  const here = d.machines[d.here]?.name ?? "This machine";
  const peerId = d.here === "a" ? "b" : "a";
  const peer = d.machines[peerId]?.name ?? "The other machine";
  const mac = d.machines[d.here]?.os === "macos";
  const g = ctx.gallery;
  const alone = d.pair.peer.kind === "unpaired";
  const peerOff = !alone && d.machines[peerId]?.power === "off";
  // No cable: over Wi-Fi or, worse, with no link at all (with the other machine off it is the card above).
  const noCable = !alone && !peerOff && d.link.kind !== "usb4" && d.link.kind !== "ethernet";
  // The card data comes from the desk; without it (gallery) the generic is named.
  const tunnel = d.tunnels.find((t) => t.state !== "closed") ?? d.tunnels[0];
  const job = tunnel ? `«${tunnel.title}»` : "The job";
  const scene = ctx.failedScene ?? (g ? { id: d.activeScene ?? d.scenes[0]?.id ?? "", name: d.scenes.find((x) => x.id === d.activeScene)?.name ?? d.scenes[0]?.name ?? "" } : null);
  const bad = d.monitors.find((m) => !m.compat.ok);
  const unreachable = d.monitors.find((m) => m.compat.ok && m.owner === null);
  const monitor = unreachable ?? d.monitors.find((m) => m.compat.ok);
  const missingPermission = d.permissions.rows.some((r) => r.state === "missing") || d.trouble === "permissions";

  const cards: (StateCardSpec | null)[] = [
    alone || g
      ? {
          id: "first",
          tag: "First time",
          tone: "blue",
          icon: "plus",
          title: "One-machine desk",
          body: "Everything works on this machine. What needs another one (shared keyboard, compute, scenes) shows as off until you pair the other machine.",
          action: { label: "Pair the other machine", icon: "plus", variant: "pri", does: "pair-peer" },
        }
      : null,
    peerOff || g
      ? {
          id: "off",
          tag: "Machine off",
          tone: "grey",
          icon: "power",
          stroke: 2.2,
          title: `${peer} is off`,
          body: `Its card is dimmed and the mouse no longer crosses to it. ${here} can wake it over the network.`,
          action: { label: "Power on", icon: "power", variant: "pri", does: "power-on", hint: WAKE_HINT },
        }
      : null,
    noCable || g
      ? {
          id: "cable",
          tag: "Link",
          tone: "amber",
          icon: "wifi-fill",
          title: "The cable was unplugged",
          body:
            d.link.kind === null && !g
              ? `There is no link with ${peer}. Keyboard and mouse do not cross until a cable or Wi-Fi comes back.`
              : "Keyboard and mouse carry on over Wi-Fi. Data transfers wait for a cable to come back, or you can send them over Wi-Fi now.",
        }
      : null,
    // Reconnection and failure of a job: the agent does not send that history yet, so only the gallery.
    g
      ? {
          id: "resumed",
          tag: "Compute",
          tone: "blue",
          icon: "sync",
          title: "Reconnected: the tunnel kept going",
          body: `${job} kept running on ${peer} while there was no link. The missing output was recovered.`,
        }
      : null,
    g
      ? {
          id: "failed",
          tag: "Compute",
          tone: "red",
          icon: "x",
          title: `${job} failed`,
          body: `It ran out of resources on ${peer}. Claude sees this reason over MCP and can wait or pick another machine.`,
          action: { label: "View in Tunnels", icon: "eye", variant: "ghost", does: "tunnels" },
        }
      : null,
    scene && (ctx.failedScene || g)
      ? {
          id: "scene",
          tag: "Monitors",
          tone: "red",
          icon: "screen",
          title: `The scene “${scene.name}” was not applied`,
          body: "A monitor answered “change failed”. Tandem rolled back the scene: everything stayed as it was.",
          action: { label: "Retry", icon: "refresh", variant: "default", does: "retry-scene", sceneId: scene.id },
        }
      : null,
    bad || g
      ? {
          id: "unsupported",
          tag: "Monitors",
          tone: "amber",
          icon: "warn",
          title: bad ? `“${bad.name}” is not supported` : "Unsupported monitor",
          body: "Tandem only controls monitors that meet the standard: they report which input they show, confirm each change and report changes made with their buttons. The others are shown but not touched.",
        }
      : null,
    unreachable || g
      ? {
          id: "nobody",
          tag: "Monitors",
          tone: "grey",
          icon: "screen",
          title: `Nobody can talk to ${monitor ? `“${monitor.name}”` : "the monitor"}`,
          body: `Its owner (${peer}) is off and ${here} reaches it only over HDMI while it is showing it. When either can, it takes control on its own.`,
        }
      : null,
    missingPermission || g
      ? {
          id: "permission",
          tag: "Permissions",
          tone: "amber",
          icon: "shield-plain",
          title: mac ? "“Input Monitoring” is missing on macOS" : "A system permission is missing",
          body: mac
            ? `Without this permission, ${here} cannot send its keyboard. Grant it to {code}; Tandem detects it on its own.`
            : `Without this permission, ${here} cannot send its keyboard. Grant it to Tandem; Tandem detects it on its own.`,
          code: mac ? shortPath(d.permissions.binary) : undefined,
          action: { label: "Open System Settings", icon: "external", variant: "pri", does: "permission" },
        }
      : null,
  ];
  return cards.filter((c): c is StateCardSpec => c !== null).map((c) => (ctx.example && EXAMPLE[c.id] ? { ...c, ...EXAMPLE[c.id] } : c));
}
