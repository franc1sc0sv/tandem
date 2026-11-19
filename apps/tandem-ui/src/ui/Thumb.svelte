<script lang="ts">
  // Thumbnail of a screen (`.thumb` + `.wall` of V3-Monitors): the background of the machine it
  // shows; with `flashKey`, a white flash on mounting and every time it changes (input change).
  interface Props {
    width: number;
    height: number;
    /** Padding between the frame and the background (5 px; 3 on the scene thumbnails). */
    pad?: number;
    /** Value of the background's `background`. */
    wall: string;
    flashKey?: string | null;
    /** Dotted, dimmed frame (incompatible monitor). */
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
