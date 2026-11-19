<script lang="ts" module>
  import type { IconName } from "./icons";

  export interface SegOption {
    value: string;
    label: string;
    icon?: IconName;
    /** Color del ícono (por ejemplo, el de la machine). */
    iconColor?: string;
    /** Texto chico al lado (`HDMI`). */
    note?: string;
    /** Borde del indicador cuando esta opción está elegida. */
    accent?: string;
    /** Grosor del trazo del ícono (1.8 por omisión; 2 en las alineaciones del arrangement). */
    iconStroke?: number;
  }
</script>

<script lang="ts">
  import Icon from "./Icon.svelte";

  interface Props {
    options: SegOption[];
    value: string;
    /** Nombre accesible del grupo. */
    label: string;
    /** `md`: 36 px por opción (Arrangement); `lg`: 42 px (Monitors). */
    size?: "md" | "lg";
    onchange?: (value: string) => void;
  }

  let { options, value = $bindable(), label, size = "md", onchange }: Props = $props();

  const index = $derived(Math.max(0, options.findIndex((o) => o.value === value)));
  const accent = $derived(options[index]?.accent);
  let buttons: HTMLButtonElement[] = $state([]);

  function pick(i: number, focus = false) {
    const o = options[i];
    if (!o) return;
    if (o.value !== value) {
      value = o.value;
      onchange?.(o.value);
    }
    if (focus) buttons[i]?.focus();
  }

  // Grupo de radio (WAI-ARIA): las flechas mueven y eligen, Inicio/Fin van a los extremos.
  function onkeydown(e: KeyboardEvent) {
    const n = options.length;
    const keys: Record<string, number> = { ArrowRight: index + 1, ArrowDown: index + 1, ArrowLeft: index - 1, ArrowUp: index - 1, Home: 0, End: n - 1 };
    const next = keys[e.key];
    if (next === undefined) return;
    e.preventDefault();
    pick((next + n) % n, true);
  }
</script>

<div class="seg {size}" role="radiogroup" aria-label={label} style="grid-template-columns:repeat({options.length},1fr);--n:{options.length}" tabindex="-1" {onkeydown}>
  <span class="ind" style="transform:translateX({index * 100}%){accent ? `;border-color:${accent}` : ''}" aria-hidden="true"></span>
  {#each options as o, i (o.value)}
    <button
      bind:this={buttons[i]}
      type="button"
      role="radio"
      class="sb"
      aria-checked={i === index}
      tabindex={i === index ? 0 : -1}
      onclick={() => pick(i)}
    >
      {#if o.icon}<Icon name={o.icon} size={size === "lg" ? 17 : 16} color={o.iconColor} stroke={o.iconStroke} />{/if}{o.label}{#if o.note}<span class="note">{o.note}</span>{/if}
    </button>
  {/each}
</div>

<style>
  /* Segmentado con indicador deslizante (V3-Arrangement, V3-Monitors). */
  .seg {
    --pad: 3px;
    position: relative;
    display: grid;
    padding: var(--pad);
    border-radius: 11px;
    background: #0f1013;
    border: 1px solid #25272d;
  }
  .seg.lg {
    --pad: 4px;
    border-radius: 12px;
  }
  .ind {
    position: absolute;
    top: var(--pad);
    bottom: var(--pad);
    left: var(--pad);
    /* Caja de contenido, como en los mockups: con el borde, el indicador mide 2 px más que
       una opción. */
    width: calc((100% - 2 * var(--pad)) / var(--n));
    border-radius: 8px;
    background: linear-gradient(180deg, #25262c, #1e1f24);
    border: 1px solid #3a3d45;
    transition: transform 0.38s cubic-bezier(0.3, 1.3, 0.5, 1);
  }
  .lg .ind {
    border-radius: 9px;
    box-shadow: 0 4px 14px rgba(0, 0, 0, 0.35);
    /* El borde toma el color de la machine elegida (V3-Monitors). */
    transition:
      transform 0.38s cubic-bezier(0.3, 1.3, 0.5, 1),
      border-color 0.3s;
  }
  .sb {
    position: relative;
    z-index: 1;
    font: inherit;
    font-size: 12.5px;
    height: 36px;
    border: 0;
    background: transparent;
    color: #9a9da6;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 7px;
    transition: color 0.25s;
  }
  .lg .sb {
    font-size: 13px;
    height: 42px;
    gap: 9px;
  }
  .sb[aria-checked="true"] {
    color: #e8e8eb;
  }
  .sb:hover {
    color: #e8e8eb;
  }
  .sb:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .note {
    font-family: "JetBrains Mono", ui-monospace, monospace;
    font-size: 10.5px;
    color: #80838d;
  }
</style>
