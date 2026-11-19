<script lang="ts" module>
  export type ChipTone = "neutral" | "ok" | "info" | "warn" | "bad" | "idle";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  interface Props {
    tone?: ChipTone;
    /** Punto de estado (el formato uniforme); `false` para un chip de solo texto o con ícono. */
    dot?: boolean;
    /** El punto late (estado vivo). */
    pulse?: boolean;
    icon?: IconName;
    /** Contador chico (el de Tunnels en la barra lateral). */
    count?: boolean;
    class?: string;
    style?: string;
    children: Snippet;
  }

  let { tone = "neutral", dot = true, pulse = false, icon, count = false, class: cls = "", style = "", children }: Props = $props();
</script>

<span class="chip {tone} {cls}" class:count class:pulse {style}>
  {#if icon}<Icon name={icon} size={13} stroke={2} />{:else if dot && !count}<span class="d" aria-hidden="true"></span>{/if}{@render children()}
</span>

<style>
  /* Chip de estado uniforme: punto (o ícono) + texto, mismo alto en todas las pantallas. */
  .chip {
    --c: #9a9da6;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 9px;
    border-radius: 999px;
    border: 1px solid #31343b;
    font-size: 12px;
    color: #c4c6cc;
    white-space: nowrap;
  }
  .chip.count {
    padding: 1px 7px;
    font-size: 11px;
  }
  .d {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--c);
    flex: none;
  }
  .pulse .d {
    animation: chip-pl 2s ease-in-out infinite;
  }
  .ok {
    --c: #6cc59a;
    --g: rgba(108, 197, 154, 0.55);
  }
  .info {
    --c: #7aa7e6;
    --g: rgba(122, 167, 230, 0.55);
    border-color: #4f7cbc;
    color: #c3d8f6;
  }
  .warn {
    --c: #d9a75a;
    --g: rgba(217, 167, 90, 0.55);
    border-color: #5a4a2c;
    color: #e6c48f;
  }
  .bad {
    --c: #e07a6f;
    --g: rgba(224, 122, 111, 0.55);
    border-color: #5a3532;
    color: #f0b1aa;
  }
  .idle {
    --c: #4a4d55;
    color: #9a9da6;
  }
  @keyframes chip-pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 var(--g, rgba(154, 157, 166, 0.5));
    }
    50% {
      box-shadow: 0 0 0 5px transparent;
    }
  }
</style>
