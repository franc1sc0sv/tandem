<script lang="ts">
  interface Props {
    /** 0–100. */
    percent: number;
    color: string;
    height?: number;
    label?: string;
  }

  let { percent, color, height = 6, label }: Props = $props();
  const w = $derived(Math.max(0, Math.min(100, percent)));
</script>

<div
  class="bar"
  style="height:{height}px"
  role={label ? "meter" : undefined}
  aria-label={label}
  aria-valuenow={label ? w : undefined}
  aria-valuemin={label ? 0 : undefined}
  aria-valuemax={label ? 100 : undefined}
>
  <span style="width:{w}%;background:{color}"></span>
</div>

<style>
  /* Barra de uso: crece al entrar y se mueve suave con cada muestra (V3-Puesto). */
  .bar {
    border-radius: 3px;
    background: #24262c;
    overflow: hidden;
  }
  span {
    display: block;
    height: 100%;
    border-radius: 3px;
    animation: bar-grow 1.1s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    transition: width 0.6s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  @keyframes bar-grow {
    from {
      width: 0;
    }
  }
</style>
