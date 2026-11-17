<script lang="ts">
  import { ICONS, type IconDef, type IconName } from "./icons";

  interface Props {
    name: IconName;
    size?: number;
    /** Grosor del trazo (1.8 en la navegación, 2 en botones y chips). */
    stroke?: number;
    color?: string;
    class?: string;
    style?: string;
  }

  let { name, size = 16, stroke = 1.8, color = "currentColor", class: cls = "", style = "" }: Props = $props();
  const def: IconDef = $derived(ICONS[name]);
</script>

<!-- El cuerpo sale del set propio (`icons.ts`), nunca de datos de afuera. -->
{#if def.fill}
  <svg class={cls} {style} width={size} height={size} viewBox="0 0 24 24" fill={color} aria-hidden="true">{@html def.body}</svg>
{:else}
  <svg
    class={cls}
    {style}
    width={size}
    height={size}
    viewBox="0 0 24 24"
    fill="none"
    stroke={color}
    stroke-width={stroke}
    stroke-linecap={def.linecap}
    stroke-linejoin={def.linejoin}
    aria-hidden="true">{@html def.body}</svg
  >
{/if}
