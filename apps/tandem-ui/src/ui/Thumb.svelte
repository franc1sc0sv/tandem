<script lang="ts">
  // Miniatura de una pantalla (`.thumb` + `.wall` de V3-Monitors): el fondo de la máquina que
  // muestra; con `flashKey`, un destello blanco al montarse y cada vez que cambia (cambio de entrada).
  interface Props {
    width: number;
    height: number;
    /** Relleno entre el marco y el fondo (5 px; 3 en las miniaturas de escena). */
    pad?: number;
    /** Valor de `background` del fondo. */
    wall: string;
    flashKey?: string | null;
    /** Marco punteado y apagado (monitor no compatible). */
    dim?: boolean;
    label?: string;
  }

  let { width, height, pad = 5, wall, flashKey = null, dim = false, label }: Props = $props();
</script>

<div class="thumb" class:dim style="width:{width}px;height:{height}px;padding:{pad}px" role={label ? "img" : undefined} aria-label={label}>
  <div class="wall" style="background:{wall}"></div>
  {#if flashKey !== null}{#key flashKey}<span class="flash" style="inset:{pad}px"></span>{/key}{/if}
</div>

<style>
  .thumb {
    border-radius: 9px;
    background: #0b0c0e;
    border: 1px solid #31343b;
    box-sizing: border-box;
    flex: none;
    position: relative;
  }
  .thumb.dim {
    opacity: 0.5;
    border-style: dashed;
  }
  .wall {
    width: 100%;
    height: 100%;
    border-radius: 4px;
    transition: background 0.7s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .flash {
    position: absolute;
    border-radius: 4px;
    background: #fff;
    opacity: 0;
    animation: thumb-fls 0.6s ease-out;
  }
  @keyframes thumb-fls {
    0% {
      opacity: 0.25;
    }
    100% {
      opacity: 0;
    }
  }
</style>
