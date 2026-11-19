<script lang="ts">
  import { ICONS, type IconDef, type IconName } from "./icons";

  interface Props {
    name: IconName;
    size?: number;
    /** Stroke thickness (1.8 in navigation, 2 in buttons and chips). */
    stroke?: number;
    color?: string;
    /** Stroke cap if the mockup asks for one different from the icon's (the "+" of "New scene" has it round). */
    linecap?: "round" | "butt";
    class?: string;
    style?: string;
  }

  let { name, size = 16, stroke = 1.8, color = "currentColor", linecap, class: cls = "", style = "" }: Props = $props();
  const def: IconDef = $derived(ICONS[name]);
</script>

<!-- The body comes from the own set (`icons.ts`), never from outside data. -->
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
    stroke-linecap={linecap ?? def.linecap}
    stroke-linejoin={def.linejoin}
    aria-hidden="true">{@html def.body}</svg
  >
{/if}
