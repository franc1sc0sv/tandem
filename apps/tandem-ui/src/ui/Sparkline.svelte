<script lang="ts" module>
  let seq = 0;

  /** Puntos de la línea en un lienzo de `width`×`height` (valores 0–100, abajo es 0). */
  export function sparkPoints(values: number[], width = 300, height = 54): [number, number][] {
    if (values.length === 0) return [];
    const step = values.length > 1 ? width / (values.length - 1) : 0;
    return values.map((v, i) => [Math.round(i * step * 100) / 100, Math.round(height - (Math.max(0, Math.min(100, v)) * height) / 100)]);
  }
</script>

<script lang="ts">
  interface Props {
    values: number[];
    color: string;
    height?: number;
    fillOpacity?: number;
    class?: string;
  }

  let { values, color, height = 54, fillOpacity = 0.28, class: cls = "" }: Props = $props();
  const id = `spark-${++seq}`;
  const pts = $derived(sparkPoints(values, 300, height));
  const line = $derived(pts.map(([x, y], i) => `${i ? "L" : "M"}${x} ${y}`).join(" "));
  const area = $derived(pts.length ? `${line} L300 ${height} L0 ${height}Z` : "");
</script>

<svg class="spark {cls}" viewBox="0 0 300 {height}" width="100%" {height} preserveAspectRatio="none" aria-hidden="true">
  <defs>
    <linearGradient {id} x1="0" x2="0" y1="0" y2="1">
      <stop offset="0" stop-color={color} stop-opacity={fillOpacity}></stop>
      <stop offset="1" stop-color={color} stop-opacity="0"></stop>
    </linearGradient>
  </defs>
  <path d={area} fill="url(#{id})"></path>
  <path class="ln" d={line} fill="none" stroke={color} stroke-width="1.5"></path>
</svg>

<style>
  .spark {
    display: block;
  }
  /* La línea se dibuja al entrar (V3-Desk). */
  .ln {
    stroke-dasharray: 600;
    stroke-dashoffset: 600;
    animation: spark-dr 1.8s ease-out 0.2s forwards;
  }
  @keyframes spark-dr {
    to {
      stroke-dashoffset: 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ln {
      stroke-dashoffset: 0;
    }
  }
</style>
