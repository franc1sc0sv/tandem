<script lang="ts" module>
  export type ButtonVariant = "default" | "pri" | "ghost" | "danger";
  /** Fixed height per size: sm 32, md 36, lg 40, xl 44 (those of the mockups). */
  export type ButtonSize = "sm" | "md" | "lg" | "xl";
  /** How the icon moves on hover: grows, advances, goes back or spins. */
  export type IconMotion = "scale" | "go" | "back" | "spin" | "lift" | "tilt";
</script>

<script lang="ts">
  import type { Snippet } from "svelte";
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  interface Props {
    /** Icon always (G4). */
    icon: IconName;
    variant?: ButtonVariant;
    size?: ButtonSize;
    /** Icon only (square): `label` is the accessible name and the title. */
    iconOnly?: boolean;
    label?: string;
    /** The icon goes after the text (for example "Next →"). */
    iconAfter?: boolean;
    motion?: IconMotion;
    /** Size and stroke of the icon if the screen's mockup does not use those of the button size. */
    iconSize?: number;
    iconStroke?: number;
    iconLinecap?: "round" | "butt";
    href?: string;
    type?: "button" | "submit";
    disabled?: boolean;
    pressed?: boolean;
    onclick?: (e: MouseEvent) => void;
    class?: string;
    style?: string;
    children?: Snippet;
  }

  let {
    icon,
    variant = "default",
    size = "md",
    iconOnly = false,
    label,
    iconAfter = false,
    motion = "scale",
    iconSize: iconPx,
    iconStroke = 2,
    iconLinecap,
    href,
    type = "button",
    disabled = false,
    pressed,
    onclick,
    class: cls = "",
    style = "",
    children,
  }: Props = $props();

  const classes = $derived(["btn", variant !== "default" && variant, size, iconOnly && "ic", `m-${motion}`, cls].filter(Boolean).join(" "));
  const iconSize = $derived(iconPx ?? (size === "sm" ? 15 : size === "xl" ? 17 : 16));
</script>

{#snippet body()}{#if !iconAfter}<Icon name={icon} size={iconSize} stroke={iconStroke} linecap={iconLinecap} />{/if}{#if !iconOnly && children}{@render children()}{/if}{#if iconAfter}<Icon
      name={icon}
      size={iconSize}
      stroke={iconStroke}
      linecap={iconLinecap}
    />{/if}{/snippet}

{#if href && !disabled}
  <a class={classes} {style} {href} aria-label={iconOnly ? label : undefined} title={iconOnly ? label : undefined} {onclick}>{@render body()}</a>
{:else}
  <button
    class={classes}
    {style}
    {type}
    {disabled}
    aria-pressed={pressed}
    aria-label={iconOnly ? label : undefined}
    title={iconOnly ? label : undefined}
    {onclick}>{@render body()}</button
  >
{/if}

<style>
  /* Single button system (v0.3 mockups): fixed height, icon always, lift on hover,
     shine on the primary and visible focus. The sizes use variables so each screen
     fits its mockup without another system. */
  .btn {
    --h: 36px;
    --px: 14px;
    --fs: 13px;
    --r: 10px;
    --gap: 8px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: var(--btn-gap, var(--gap));
    height: var(--btn-h, var(--h));
    min-height: 0;
    padding: 0 var(--btn-px, var(--px));
    box-sizing: border-box;
    flex: none;
    align-self: center;
    white-space: nowrap;
    font: inherit;
    font-size: var(--btn-fs, var(--fs));
    font-weight: 500;
    /* The Monitors and scenes mockups do not set the button's line height: `normal`. */
    line-height: var(--btn-lh, 1);
    border-radius: var(--btn-r, var(--r));
    border: 1px solid #2e3138;
    background: linear-gradient(180deg, #1f2025, #18191d);
    color: #e8e8eb;
    cursor: pointer;
    text-decoration: none;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.05),
      0 1px 2px rgba(0, 0, 0, 0.3);
    transition:
      transform 0.18s cubic-bezier(0.2, 0.8, 0.2, 1),
      border-color 0.18s,
      box-shadow 0.18s;
    position: relative;
    overflow: hidden;
  }
  .btn.sm {
    --h: 32px;
    --px: 11px;
    --fs: 12.5px;
    --r: 9px;
  }
  .btn.lg {
    --h: 40px;
    --px: 16px;
    --fs: 13.5px;
  }
  .btn.xl {
    --h: 44px;
    --px: 20px;
    --fs: 14px;
    --r: 11px;
    --gap: 9px;
  }
  .btn.ic {
    width: var(--btn-h, var(--h));
    padding: 0;
  }
  .btn :global(svg) {
    flex: none;
    transition: transform 0.25s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .btn:hover:not(:disabled) {
    border-color: #41454e;
    transform: translateY(-1px);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.06),
      0 6px 16px rgba(0, 0, 0, 0.35);
  }
  .btn.m-scale:hover:not(:disabled) :global(svg) {
    transform: scale(1.14);
  }
  .btn.m-go:hover:not(:disabled) :global(svg) {
    transform: translateX(4px);
  }
  .btn.m-back:hover:not(:disabled) :global(svg) {
    transform: translateX(-3px);
  }
  .btn.m-spin:hover:not(:disabled) :global(svg) {
    transform: rotate(-180deg);
  }
  .btn.m-lift:hover:not(:disabled) :global(svg) {
    transform: translate(2px, -2px);
  }
  /* The gauge tilts like a needle (`Measure bandwidth`, V3-Network). */
  .btn.m-tilt:hover:not(:disabled) :global(svg) {
    transform: rotate(-25deg) scale(1.1);
  }
  .btn:active:not(:disabled) {
    transform: scale(0.97);
  }
  .btn:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .btn:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .btn.pri {
    background: linear-gradient(180deg, #3a6aa8, #2c5288);
    border-color: #5b8fd4;
    color: #fff;
  }
  .btn.pri:hover:not(:disabled) {
    border-color: #5b8fd4;
    box-shadow:
      0 0 0 4px rgba(122, 167, 230, 0.16),
      0 8px 20px rgba(44, 82, 136, 0.45);
  }
  .btn.pri::after {
    content: "";
    position: absolute;
    inset: 0;
    background: linear-gradient(110deg, transparent 30%, rgba(255, 255, 255, 0.2) 50%, transparent 70%);
    transform: translateX(-120%);
    transition: transform 0.6s;
    pointer-events: none;
  }
  .btn.pri:hover:not(:disabled)::after {
    transform: translateX(120%);
  }
  .btn.ghost {
    background: transparent;
    border-color: transparent;
    box-shadow: none;
    color: #9a9da6;
  }
  .btn.ghost:hover:not(:disabled) {
    background: #1f2026;
    color: #e8e8eb;
    border-color: transparent;
    box-shadow: none;
  }
  .btn.danger {
    color: #f0b1aa;
    border-color: #5a3532;
    background: linear-gradient(180deg, #24191a, #1c1516);
  }
  .btn.danger:hover:not(:disabled) {
    border-color: #8a4c46;
    box-shadow: 0 0 0 4px rgba(224, 122, 111, 0.12);
  }
</style>
