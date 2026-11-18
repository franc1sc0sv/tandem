<script lang="ts">
  // Acomodo (V3-Acomodo, D18): las pantallas a escala física, centradas solas; el panel de la
  // elegida con sus datos, el mini plano de 4 casillas para la otra máquina y la alineación.
  // Sin X/Y. «Guardar» es un cambio del puesto compartido (T-099).
  import { ALIGNS, SIDES, SIDE_LABEL, SLOTS, alignLabels, draw, fillOf, initialSelection, isVertical, panelFor, placeOther, wallOf, type Align, type Arrangement, type LayoutScreen, type Side } from "../lib/acomodo";
  import { useStore } from "../lib/context";
  import { formatHash } from "../lib/routes";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";
  import type { IconName } from "../ui/icons";
  import Segmented, { type SegOption } from "../ui/Segmented.svelte";

  const store = useStore();
  const desk = $derived(store.desk);

  /** Lo que el usuario movió y todavía no guardó; `null`: lo del puesto. */
  let draft: Arrangement | null = $state(null);
  let picked: string | null = $state(null);
  let saving = $state(false);
  /** El acomodo del puesto cuando se guardó, mientras no llegue el nuevo: el borrador se queda. */
  let awaiting: Arrangement | null = $state(null);

  const arrangement = $derived(draft ?? desk.layout.arrangement);
  const layout = $derived({ screens: desk.layout.screens, arrangement });
  const drawing = $derived(draw(layout));
  const selected = $derived(picked !== null && layout.screens.some((s) => s.id === picked) ? picked : initialSelection(layout, desk.here));
  const panel = $derived(selected ? panelFor(layout, selected) : null);
  const byId = $derived(new Map(layout.screens.map((s) => [s.id, s])));

  const scene = $derived(desk.scenes.find((s) => s.id === desk.activeScene) ?? null);
  const machines = $derived((["a", "b"] as const).flatMap((m) => (desk.machines[m] ? [desk.machines[m]] : [])));
  const colorOf = (s: LayoutScreen) => (s.machine ? (desk.machines[s.machine]?.color ?? "#9a9da6") : "#6a6d76");

  const ALIGN_ICON: Record<"v" | "h", Record<Align, IconName>> = {
    v: { start: "align-top", center: "align-middle", end: "align-bottom" },
    h: { start: "align-left", center: "align-center", end: "align-right" },
  };
  const SLOT_ICON: Record<Side, IconName> = { up: "arrow-up", down: "arrow-down", left: "arrow-left", right: "arrow-right" };
  const alignOptions: SegOption[] = $derived.by(() => {
    const labels = alignLabels(arrangement.aSide);
    const icons = ALIGN_ICON[isVertical(arrangement.aSide) ? "v" : "h"];
    return ALIGNS.map((a) => ({ value: a, label: labels[a], icon: icons[a], iconStroke: 2 }));
  });

  function zOf(s: LayoutScreen): number {
    return !s.usable ? 0 : s.machine === "a" ? 2 : 1;
  }

  function boxStyle(id: string): string {
    const b = drawing.boxes.find((x) => x.id === id);
    const s = byId.get(id);
    if (!b || !s) return "";
    const pos = `left:${b.left}%;top:${b.top}%;width:${b.width}%;height:${b.height}%;z-index:${zOf(s)};`;
    if (!s.usable) return `${pos}border:1.5px dashed #6a6d76;background:transparent;opacity:.4;`;
    return `${pos}border:1.5px solid ${colorOf(s)};background:${fillOf(colorOf(s))};`;
  }

  const edgeStyle = $derived.by(() => {
    const e = drawing.edge;
    if (!e) return "";
    return e.vertical ? `left:calc(${e.left}% - 1.5px);top:${e.top}%;width:3px;height:${e.length}%` : `left:${e.left}%;top:calc(${e.top}% - 1.5px);height:3px;width:${e.length}%`;
  });

  const same = (x: Arrangement, y: Arrangement) => x.aSide === y.aSide && x.align === y.align;

  function place(slot: Side) {
    const m = panel?.screen.machine;
    if (!m) return;
    awaiting = null;
    draft = placeOther(arrangement, m, slot);
  }

  function setAlign(v: string) {
    const align = ALIGNS.find((a) => a === v);
    if (!align) return;
    awaiting = null;
    draft = { ...arrangement, align };
  }

  async function save() {
    saving = true;
    const before = desk.layout.arrangement;
    const mine = arrangement;
    const r = await store.setArrangement(mine);
    saving = false;
    if (!r.ok) return;
    // El agente responde antes de que llegue el `Desk` nuevo: hasta entonces se sigue viendo lo guardado.
    const now = desk.layout.arrangement;
    if (!same(now, before) || same(now, mine)) draft = null;
    else awaiting = before;
  }

  // Llegó otro acomodo del puesto (el guardado, o uno de la otra máquina): el borrador ya no hace falta.
  $effect(() => {
    const now = desk.layout.arrangement;
    if (awaiting && !same(now, awaiting)) {
      draft = null;
      awaiting = null;
    }
  });

  const crossHref = formatHash({ screen: "cruce" });
</script>

<div class="page">
  <PageHeader title="Acomodo" search={false}>
    {#snippet actions()}
      {#if scene}
        <Chip dot={false} style="height:30px;box-sizing:border-box"
          ><Icon name="play" size={13} color={desk.machines[scene.shows]?.color ?? "#9a9da6"} />{scene.name}</Chip
        >
      {/if}
      <Button icon="cursor" href={crossHref} disabled={!store.can("crossing")} style="--btn-h:38px;--btn-px:15px">Probar cruce</Button>
      <Button icon="check" variant="pri" disabled={saving} onclick={save} style="--btn-h:38px;--btn-px:15px">Guardar</Button>
    {/snippet}
  </PageHeader>

  <div class="row">
    <Card tag="section" label="Lienzo del acomodo" class="canvas">
      <div class="board">
        <div class="stage" style="aspect-ratio:{drawing.aspect}">
          {#each layout.screens as s (s.id)}
            <button type="button" class="scr" class:on={s.id === selected} style={boxStyle(s.id)} aria-pressed={s.id === selected} onclick={() => (picked = s.id)}>
              <b>{s.name}</b>
              <span class="mono res">{s.usable ? s.res : "desconectado"}</span>
            </button>
          {/each}
          {#if drawing.edge}<div class="xedge" style={edgeStyle}></div>{/if}
        </div>
        <div class="legend">
          {#each machines as m (m.id)}
            <span class="li"><span class="sw" style="border-color:{m.color}"></span>{m.name}</span>
          {/each}
          {#if drawing.edge}<span class="li"><span class="ln"></span>Borde de cruce</span>{/if}
        </div>
      </div>
    </Card>

    {#if panel}
      {@const s = panel.screen}
      {#key s.id}
        <Card tag="section" label="Pantalla seleccionada" class="panel">
          <div class="who">
            <div class="thumb"><div style="background:{s.usable ? wallOf(colorOf(s)) : 'repeating-linear-gradient(45deg,#17181c 0 6px,#121316 6px 12px)'}"></div></div>
            <div class="names">
              <b>{s.name}</b>
              <span class="mut line"><span class="dot" style="background:{s.usable ? colorOf(s) : '#4a4d55'}"></span>{s.line}</span>
            </div>
          </div>

          <div class="facts">
            <div class="fact">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="3" y="5" width="18" height="12" rx="2"></rect><path d="M8 21h8"></path></svg><span
                class="mut k">Tamaño</span
              ><span class="mono">{s.size}</span>
            </div>
            <div class="fact">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M4 8V4h4M20 8V4h-4M4 16v4h4M20 16v4h-4"></path></svg><span class="mut k"
                >Resolución</span
              ><span class="mono">{s.res}</span>
            </div>
            <div class="fact">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M21 21H3V3"></path><path d="M7 14l4-4 3 3 5-6"></path></svg><span class="mut k"
                >Proporción</span
              ><span class="mono">{s.ratio}</span>
            </div>
            <div class="fact">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><circle cx="11" cy="11" r="7"></circle><path d="m20 20-3.5-3.5M8 11h6M11 8v6"></path></svg><span
                class="mut k">Escala</span
              ><span class="mono">{s.scale}</span>
            </div>
          </div>

          {#if panel.other}
            {@const o = panel.other}
            {@const slot = SLOTS[panel.shown]}
            <div class="group">
              <span class="lbl">{panel.question}</span>
              <div class="pad" role="radiogroup" aria-label={panel.question}>
                <div class="ref" style="border:1.5px solid {colorOf(s)};background:{fillOf(colorOf(s))}">{s.short}</div>
                {#each SIDES as side (side)}
                  {@const p = SLOTS[side]}
                  <button
                    type="button"
                    role="radio"
                    class="slot"
                    style="left:{p.left}%;top:{p.top}%;width:{p.width}%;height:{p.height}%"
                    aria-label={SIDE_LABEL[side]}
                    aria-checked={panel.shown === side}
                    onclick={() => place(side)}><Icon name={SLOT_ICON[side]} size={15} stroke={2.2} /></button
                  >
                {/each}
                <div class="mpos" style="left:{slot.left}%;top:{slot.top}%;width:{slot.width}%;height:{slot.height}%;border:1.5px solid {colorOf(o)};background:{fillOf(colorOf(o))}">
                  {o.short}
                </div>
              </div>
            </div>
            <div class="group">
              <span class="lbl">Alineación</span>
              <Segmented options={alignOptions} value={arrangement.align} label="Alineación" onchange={setAlign} />
            </div>
          {:else if !s.usable}
            <div class="locked"><Icon name="warn" size={16} stroke={2} />No compatible: no entra en el acomodo.</div>
          {:else}
            <div class="locked"><Icon name="warn" size={16} stroke={2} />La otra máquina no tiene pantallas conectadas: no hay acomodo que editar.</div>
          {/if}
        </Card>
      {/key}
    {/if}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .row {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    align-items: stretch;
  }
  .page :global(.card.canvas) {
    flex: 999 1 562px;
    min-width: 0;
    padding: 0;
    animation: none;
  }
  .board {
    position: relative;
    height: 100%;
    min-height: 440px;
    background-color: #0b0c0e;
    background-image: radial-gradient(#1c1e23 1px, transparent 1px);
    background-size: 20px 20px;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 18px 30px;
    box-sizing: border-box;
  }
  .stage {
    position: relative;
    width: 100%;
  }
  /* Curva con rebote del mockup: las pantallas y el borde se deslizan al nuevo acomodo. */
  .scr {
    position: absolute;
    box-sizing: border-box;
    border-radius: 8px;
    font: inherit;
    color: #e8e8eb;
    cursor: grab;
    display: flex;
    flex-direction: column;
    justify-content: flex-end;
    align-items: flex-start;
    padding: 10px;
    text-align: left;
    gap: 2px;
    overflow: hidden;
    line-height: 1.3;
    transition:
      left 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      top 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      width 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      height 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      box-shadow 0.25s;
  }
  .scr:hover {
    box-shadow: 0 0 0 3px rgba(232, 232, 235, 0.1);
  }
  .scr.on {
    box-shadow: 0 0 0 3px rgba(232, 232, 235, 0.18);
  }
  .scr:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .scr b,
  .scr span {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }
  .scr b {
    font-weight: 600;
    font-size: 13px;
  }
  .res {
    font-size: 11px;
    color: #c4c6cc;
  }
  .xedge {
    position: absolute;
    background: #6cc59a;
    border-radius: 2px;
    transition:
      left 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      top 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      width 0.5s cubic-bezier(0.3, 1.2, 0.5, 1),
      height 0.5s cubic-bezier(0.3, 1.2, 0.5, 1);
    animation: gl 1.6s ease-in-out infinite;
  }
  .legend {
    position: absolute;
    left: 16px;
    bottom: 14px;
    display: flex;
    gap: 14px;
    font-size: 11.5px;
    color: #9a9da6;
  }
  .li {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .sw {
    width: 10px;
    height: 10px;
    border-radius: 3px;
    border: 1.5px solid;
    box-sizing: content-box;
  }
  .ln {
    width: 10px;
    height: 3px;
    border-radius: 2px;
    background: #6cc59a;
  }

  .page :global(.card.panel) {
    flex: 1 1 314px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 18px;
    animation: fd 0.35s ease-out;
  }
  .who {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .thumb {
    width: 54px;
    height: 30px;
    border-radius: 6px;
    padding: 3px;
    background: #0b0c0e;
    border: 1px solid #31343b;
    box-sizing: border-box;
    flex: none;
  }
  .thumb > div {
    width: 100%;
    height: 100%;
    border-radius: 3px;
  }
  .names {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }
  .names b {
    font-weight: 600;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
  .mut {
    color: #9a9da6;
  }
  .mono {
    font-family: "JetBrains Mono", ui-monospace, monospace;
  }
  .facts {
    display: flex;
    flex-direction: column;
  }
  .fact {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    padding: 8px 0;
    border-top: 1px solid #1f2126;
  }
  .fact:first-child {
    border-top: 0;
  }
  .fact :global(svg) {
    width: 15px;
    height: 15px;
    color: #80838d;
    flex: none;
  }
  .k {
    flex: 1;
  }
  .group {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .lbl {
    font-size: 12px;
    color: #9a9da6;
  }
  .pad {
    position: relative;
    height: 150px;
    border-radius: 12px;
    background-color: #0f1013;
    background-image: radial-gradient(#1a1c21 1px, transparent 1px);
    background-size: 12px 12px;
    border: 1px solid #25272d;
  }
  .ref {
    position: absolute;
    left: 31%;
    top: 35%;
    width: 38%;
    height: 30%;
    box-sizing: border-box;
    border-radius: 6px;
    display: grid;
    place-items: center;
    font-size: 11.5px;
    font-weight: 600;
    color: #e8e8eb;
  }
  .slot {
    position: absolute;
    box-sizing: border-box;
    border: 1.5px dashed #31343b;
    border-radius: 6px;
    background: transparent;
    cursor: pointer;
    color: #6a6d76;
    display: grid;
    place-items: center;
    padding: 0;
    transition:
      border-color 0.2s,
      color 0.2s,
      background 0.2s,
      transform 0.25s cubic-bezier(0.3, 1.5, 0.5, 1);
  }
  .slot:hover {
    border-color: #5b8fd4;
    color: #c3d8f6;
    background: rgba(122, 167, 230, 0.07);
    transform: scale(1.05);
  }
  .slot:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  /* La ficha de la otra máquina se desliza a la casilla elegida. */
  .mpos {
    position: absolute;
    box-sizing: border-box;
    border-radius: 6px;
    display: grid;
    place-items: center;
    font-size: 11px;
    font-weight: 600;
    color: #fff;
    pointer-events: none;
    transition:
      left 0.45s cubic-bezier(0.3, 1.3, 0.5, 1),
      top 0.45s cubic-bezier(0.3, 1.3, 0.5, 1),
      width 0.45s cubic-bezier(0.3, 1.3, 0.5, 1),
      height 0.45s cubic-bezier(0.3, 1.3, 0.5, 1);
    animation: gl2 2s ease-in-out infinite;
  }
  .locked {
    display: flex;
    gap: 10px;
    align-items: center;
    padding: 12px 14px;
    border-radius: 10px;
    border: 1px dashed #4a3e26;
    color: #e6c48f;
    font-size: 12.5px;
  }
  @keyframes gl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(108, 197, 154, 0.5);
    }
    50% {
      box-shadow: 0 0 12px 3px rgba(108, 197, 154, 0.35);
    }
  }
  @keyframes gl2 {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(122, 167, 230, 0.4);
    }
    50% {
      box-shadow: 0 0 0 5px rgba(122, 167, 230, 0);
    }
  }
  @keyframes fd {
    from {
      opacity: 0;
      transform: translateX(6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
</style>
