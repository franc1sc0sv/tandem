<script lang="ts">
  interface Props {
    /** 0–1. */
    value: number;
    color: string;
    size?: number;
    label?: string;
  }

  let { value, color, size = 54, label }: Props = $props();

  // r = 18 en un viewBox de 44 (V3-Red): la circunferencia es ≈ 113.
  const CIRC = 113;
  const offset = $derived(Math.round(CIRC * (1 - Math.max(0, Math.min(1, value)))));
</script>

<svg
  class="ring"
  width={size}
  height={size}
  viewBox="0 0 44 44"
  role={label ? "img" : undefined}
  aria-label={label}
  aria-hidden={label ? undefined : "true"}
  style="--off:{offset}"
>
  <circle cx="22" cy="22" r="18" fill="none" stroke="#24262c" stroke-width="4"></circle>
  <circle class="v" cx="22" cy="22" r="18" fill="none" stroke={color} stroke-width="4" stroke-linecap="round" stroke-dasharray={CIRC} stroke-dashoffset={offset}></circle>
</svg>

<style>
  .ring {
    transform: rotate(-90deg);
    flex: none;
  }
  .v {
    animation: ring-in 1.4s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    transition: stroke-dashoffset 0.6s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes ring-in {
    from {
      stroke-dashoffset: 113;
    }
  }
</style>
