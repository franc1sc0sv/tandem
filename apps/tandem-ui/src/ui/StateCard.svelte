<script lang="ts">
  // Tarjeta de un estado difícil (`V3-Estados`): qué pasó, qué hizo Tandem solo y qué puede
  // hacer el usuario. La usa el catálogo de Ajustes → Avanzado y el aviso del Puesto.
  import { TONE_STYLE, type StateCardSpec } from "../lib/states";
  import Button from "./Button.svelte";
  import Icon from "./Icon.svelte";

  interface Props {
    spec: StateCardSpec;
    onaction?: () => void;
    /** Posición en el catálogo: escalona la entrada (`.04 s` por tarjeta). */
    index?: number;
  }

  let { spec, onaction, index = 0 }: Props = $props();
  const tone = $derived(TONE_STYLE[spec.tone]);
  const parts = $derived(spec.code ? spec.body.split("{code}") : [spec.body]);
</script>

<article class="st" style:animation-delay="{index * 0.04}s">
  <span class="tag">{spec.tag}</span>
  <div class="hd">
    <span class="ic" style:background={tone.bg}><Icon name={spec.icon} size={16} stroke={spec.stroke ?? 2} color={tone.fg} /></span>{spec.title}
  </div>
  <p>
    {#if spec.code}{parts[0]}<span class="mono code">{spec.code}</span>{parts[1]}{:else}{spec.body}{/if}
  </p>
  {#if spec.mono}<span class="mono mut line">{spec.mono}</span>{/if}
  {#if spec.action}
    <div class="act">
      <Button icon={spec.action.icon} variant={spec.action.variant} motion={spec.action.icon === "refresh" ? "spin" : "scale"} style="--btn-h:38px;--btn-px:{spec.action.variant === 'ghost' ? 10 : 15}px" onclick={onaction}>{spec.action.label}</Button>
      {#if spec.action.hint}<span class="mut hint">{spec.action.hint}</span>{/if}
    </div>
  {/if}
</article>

<style>
  .st {
    background: #15161a;
    border: 1px solid #25272d;
    border-radius: 14px;
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    animation: up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    transition:
      transform 0.25s,
      border-color 0.25s,
      box-shadow 0.25s;
  }
  .st:hover {
    transform: translateY(-3px);
    border-color: #3a3d45;
    box-shadow: 0 12px 26px rgba(0, 0, 0, 0.3);
  }
  .tag {
    font-size: 11px;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: #80838d;
  }
  .hd {
    display: flex;
    align-items: center;
    gap: 10px;
    font-weight: 600;
    font-size: 14.5px;
  }
  .ic {
    width: 30px;
    height: 30px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    flex: none;
    transition: transform 0.3s cubic-bezier(0.3, 1.5, 0.5, 1);
  }
  .st:hover .ic {
    transform: scale(1.12) rotate(-6deg);
  }
  p {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
    color: #c4c6cc;
  }
  .code {
    font-size: 12px;
  }
  .line {
    font-size: 11.5px;
  }
  .act {
    display: flex;
    gap: 8px;
  }
  .hint {
    font-size: 12px;
    align-self: center;
  }
</style>
