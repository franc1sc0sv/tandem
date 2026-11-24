<script lang="ts">
  // Desk (home, `V3-Desk`): the two machines, the live link, the desktops at real scale
  // with the live cursor (only with the window visible) and the scenes.
  import { onMount, untrack } from "svelte";
  import { useStore } from "../lib/context";
  import { wallOf } from "../lib/arrangement";
  import { cursorPoint, flowSeconds, RateMeter, sceneOf, type Point } from "../lib/desk";
  import { isDim, linkText, shortMonitor } from "../lib/monitors";
  import { linkFigures } from "../lib/network";
  import Button from "../ui/Button.svelte";
  import type { Machine } from "../lib/model";
  import { LINK_NAME } from "../lib/machines";
  import PageHeader from "../shell/PageHeader.svelte";
  import Bar from "../ui/Bar.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";
  import { machineIcon } from "../ui/icons";
  import Kbd from "../ui/Kbd.svelte";
  import Sparkline from "../ui/Sparkline.svelte";
  import StateCard from "../ui/StateCard.svelte";
  import { stateCards } from "../lib/states";

  const store = useStore();
  const desk = $derived(store.desk);
  const list = $derived((["a", "b"] as Machine[]).map((id) => desk.machines[id]).filter((m) => m !== null));
  const a = $derived(desk.machines.a);
  const b = $derived(desk.machines.b);
  const link = $derived(desk.link);
  const linkName = $derived(link.kind ? LINK_NAME[link.kind] : "No link");
  // Real measurements only; what was not measured yet reads 0, never a dash (T-356).
  const figures = $derived(linkFigures(link));
  const AMBER = "#d9a75a";

  // The desktops at physical scale, from the same calculation as the arrangement.
  const scene = $derived(
    a && b
      ? sceneOf(desk.layout, { a: a.name, b: b.name }, { a: a.color, b: b.color })
      : null,
  );

  // The live cursor: it is requested from the agent only while the window is visible.
  let visible = $state(typeof document === "undefined" || document.visibilityState !== "hidden");
  let point = $state<Point | null>(null);
  let flashes = $state(0);
  let rate = $state(0);
  const meter = new RateMeter();
  const canCursor = $derived(store.can("cursor"));
  const flow = $derived(flowSeconds(rate));

  onMount(() => {
    const onVis = () => (visible = document.visibilityState !== "hidden");
    document.addEventListener("visibilitychange", onVis);
    return () => document.removeEventListener("visibilitychange", onVis);
  });

  // One fingerprint per value: the effect is only redone if the geometry changes, not on every desk snapshot
  // (the demo publishes a new `desk` on every tick and `watchCursor` would restart the path).
  const geometry = $derived(scene ? JSON.stringify([scene.desks, scene.feet, scene.edge, scene.aspect, desk.layout]) : "");

  $effect(() => {
    if (!visible || !geometry || !canCursor) return;
    const layout = untrack(() => desk.layout);
    const scn = untrack(() => scene);
    let stop: (() => void) | undefined;
    let gone = false;
    const tick = setInterval(() => (rate = meter.rate(Date.now())), 500);
    store
      .watchCursor((e) => {
        if (e.kind === "crossed") {
          flashes += 1;
          return;
        }
        meter.push(Date.now());
        const p = scn ? cursorPoint(scn, layout, e.sample) : null;
        if (p) point = p;
      })
      .then((f) => {
        if (gone) f();
        else stop = f;
      })
      // Without a live cursor (the agent does not offer it): the desktop is seen without a dot.
      .catch(() => {});
    return () => {
      gone = true;
      stop?.();
      clearInterval(tick);
      rate = 0;
    };
  });

  // Each monitor's link in plain words (T-349): all of them for the accessible description, and
  // those that show neither machine as a line on the card.
  const nameOf = (m: Machine) => desk.machines[m]?.name ?? m.toUpperCase();
  const monitorStates = $derived(desk.monitors.flatMap((m) => {
    const text = linkText(m.link, nameOf);
    return text ? [{ id: m.id, size: shortMonitor(m.name, m.sizeMm), text, dim: isDim(m.link) }] : [];
  }));
  const spokenStates = $derived(monitorStates.map((x) => ` ${x.size}: ${x.text}.`).join(""));

  // `V3-Desk` writes "34" shows PC-Desktop"; `V3-Monitors` (and `scenes.ts`) use "→".
  const sceneLine = (desc: string) => desc.replaceAll(" → ", " shows ");

  // Another machine looks for a pair while the paired one is silent (T-345): forgetting the old one asks first.
  let confirmOffer = $state(false);
  const offer = $derived(desk.pair.pairing.kind === "offered" ? (stateCards(desk, { gallery: false, failedScene: null, confirmOffer }).find((c) => c.id === "offer") ?? null) : null);
  function forgetAndPair() {
    if (!confirmOffer) {
      confirmOffer = true;
      return;
    }
    confirmOffer = false;
    void store.forgetAndPair();
  }

  // Power of the other machine (transaction: the agent reports each step).
  const peer = $derived(list.find((m) => !m.here) ?? null);
  const showPower = $derived(peer !== null && store.can("power") && peer.power !== "lost");
  let pending = $state<"on" | "off" | null>(null);
  async function power(on: boolean) {
    if (!peer || pending) return;
    pending = on ? "on" : "off";
    try {
      await store.setPower(peer.id, on);
    } finally {
      pending = null;
    }
  }
</script>

<div class="page">
  <PageHeader title="Desk">
    {#snippet actions()}
      {#if desk.locked}<Chip tone="warn" icon="lock">Cursor locked here</Chip>{/if}
    {/snippet}
  </PageHeader>

  {#if offer}
    <section aria-label="Pairing offer" class="offer"><StateCard spec={offer} onaction={forgetAndPair} /></section>
  {/if}

  <section aria-label="Machines" class="machines">
    {#each list as m, i (m.id)}
      <div class="mslot">
      <Card href="#/machine/{m.id}" delay={i * 0.06} class="mcard">
        <div class="mhead">
          <span class="nicon"><Icon name={machineIcon(m.os)} size={20} color={m.color} /></span>
          <div class="mname"><b>{m.name}</b><span class="mut sm">{m.detail}</span></div>
          <span class="fill"></span><span class="on-dot" class:off={m.power !== "on"} title={m.power === "on" ? "On" : "Off"}></span>
        </div>
        {#if m.resources}
          {@const r = m.resources}
          <div class="usage">
            <span class="mut">CPU</span><Bar percent={r.cpu.percent} color={m.color} /><span class="mono mut num">{r.cpu.label}</span>
            <span class="mut">RAM</span><Bar percent={r.ram.percent} color={m.color} /><span class="mono mut num">{r.ram.label}</span>
            {#if r.gpu}
              <span class="mut">{r.gpuLabel}</span><Bar percent={r.gpu.percent} color={r.gpu.percent >= 85 ? AMBER : m.color} /><span class="mono mut num">{r.gpu.label}</span>
            {/if}
          </div>
          <div class="spark"><Sparkline values={r.history} color={m.color} /></div>
        {/if}
      </Card>
      {#if showPower && peer && m.id === peer.id}
        <!-- Outside the card's link (a button inside an <a> is not valid), on top of its header. -->
        <div class="pwr" role="group" aria-label="Power of {peer.name}">
          {#if peer.power === "on"}
            <Button icon="power" variant="ghost" size="sm" iconOnly label={pending === "off" ? "Powering off…" : "Power off"} disabled={pending !== null} onclick={() => power(false)} />
          {:else}
            <Button icon="power" variant="ghost" size="sm" iconOnly label={pending === "on" || peer.power === "booting" ? "Powering on…" : "Power on"} disabled={pending !== null || peer.power === "booting"} onclick={() => power(true)} />
          {/if}
        </div>
      {/if}
      </div>
    {/each}
  </section>

  {#if a && b}
    <Card href="#/network" delay={0.16} class="link" label="Link between {a.name} and {b.name}: {linkName}">
      <span class="node"><span class="ndot" style="background:{a.color}"></span><span class="nt">{a.name}</span></span>
      <svg viewBox="0 0 600 40" preserveAspectRatio="none" class="wire" aria-hidden="true">
        <defs><linearGradient id="lk" x1="0" x2="1"><stop offset="0" stop-color={a.color}></stop><stop offset="1" stop-color={b.color}></stop></linearGradient></defs>
        <line x1="0" y1="14" x2="600" y2="14" stroke="#24262c" stroke-width="2"></line>
        {#if link.kind}<line class="flow" style="animation-duration:{flow}s" x1="0" y1="14" x2="600" y2="14" stroke="url(#lk)" stroke-width="3"></line>{/if}
        <line x1="0" y1="30" x2="600" y2="30" stroke="#24262c" stroke-width="1.5" stroke-dasharray="3 6"></line>
        {#if link.kind}<line class="flow r" style="animation-duration:{flow * 1.3}s" x1="0" y1="30" x2="600" y2="30" stroke={b.color} stroke-opacity=".35" stroke-width="1.5"></line>{/if}
      </svg>
      <span class="node"><span class="nt">{b.name}</span><span class="ndot" style="background:{b.color}"></span></span>
      <span class="sep"></span>
      <span class="stats" title={figures.title || undefined}>
        <span class="stat w130">
          <span class="st"><Icon name={link.kind === "wifi" ? "wifi" : "bolt"} size={13} color={a.color} />{linkName}</span>
          <span class="mono mut ss">{figures.rate}</span>
        </span>
        <span class="stat w110">
          <span class="st">Input</span>
          <span class="mono mut ss">{figures.input}</span>
        </span>
      </span>
    </Card>
  {/if}

  <section class="bottom">
    <Card delay={0.2} class="desks">
      <div class="ch"><h2 class="h3">Live desktops</h2><span class="fill"></span>{#each monitorStates.filter((x) => x.dim) as x (x.id)}<span class="mstate" role="status"><Icon name="power" size={13} stroke={2} />{x.size} · {x.text}</span>{/each}<a href="#/arrangement" class="ib" aria-label="Edit arrangement" title="Edit arrangement"><Icon name="edit" /></a></div>
      <div class="canvas">
        {#if scene && a && b}
          <div role="img" aria-label="To scale: {scene.desks.map((k) => k.label).join(' and ')}. The cursor moves between the two.{spokenStates}" class="scale" style="aspect-ratio:{scene.aspect}">
            {#each scene.desks as k (k.id)}
              <div class="desk" class:dim={k.dim} style="left:{k.left}%;top:{k.top}%;width:{k.width}%;height:{k.height}%"><div class="w" style="background:{k.wall}" title={k.label}><span class="wl">{k.label}</span></div></div>
            {/each}
            {#each scene.feet as f, i (i)}
              <div class="foot {f.shape ?? ''}" style="left:{f.left}%;top:{f.top}%;width:{f.width}%;height:{f.height}%;border-radius:0 0 {f.radius}px {f.radius}px"></div>
            {/each}
            {#if scene.edge}
              {@const e = scene.edge}
              {#key flashes}
                <span
                  class="edge"
                  class:on={flashes > 0}
                  style={e.vertical ? `left:calc(${e.left}% - 1px);top:${e.top}%;width:2px;height:${e.length}%` : `left:${e.left}%;top:calc(${e.top}% - 1px);width:${e.length}%;height:2px`}
                ></span>
              {/key}
            {/if}
            {#if point}<span class="curdot" style="left:{point.left}%;top:{point.top}%"></span>{/if}
          </div>
        {/if}
      </div>
    </Card>

    <Card delay={0.24} class="scenes">
      <div class="ch g4">
        <h2 class="h3">Scenes</h2><span class="fill"></span>
        <a href="#/monitors" class="ib" aria-label="New scene" title="New scene"><Icon name="plus" /></a>
        <a href="#/monitors" class="ib" aria-label="Edit scenes" title="Edit scenes"><Icon name="edit" /></a>
      </div>
      <div class="slist">
        {#each desk.scenes as s (s.id)}
          {@const on = desk.activeScene === s.id}
          {@const shows = s.shows ? desk.machines[s.shows] : null}
          <button type="button" class="scene" class:act={on} aria-pressed={on} title="{s.name}: {sceneLine(s.desc)}" onclick={() => store.applyScene(s.id)}>
            <span class="srow">
              <span class="thumb"><div style="background:{wallOf(shows?.color ?? "#7aa7e6")}"></div></span>
              <span class="stext"><b>{s.name}</b>{#each s.lines ?? [s.desc] as line, k (k)}<span class="mut sd">{sceneLine(line)}</span>{/each}</span>
              {#if s.key}<span class="skey"><Kbd>{s.key}</Kbd></span>{/if}
            </span>
          </button>
        {/each}
      </div>
    </Card>
  </section>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .fill {
    flex: 1;
  }
  .sm {
    font-size: 12px;
  }
  /* Two machines (D1): they share the row and fill it, as wide as the cards below (T-356). */
  .page {
    container: desk / inline-size;
  }
  .machines {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 14px;
  }
  @container desk (max-width: 620px) {
    .machines {
      grid-template-columns: minmax(0, 1fr);
    }
    .page :global(.card.link) {
      flex-wrap: wrap;
    }
    .sep {
      display: none;
    }
    .stats {
      flex-basis: 100%;
    }
  }
  .mslot > :global(.card.mcard) {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding-bottom: 0;
  }
  .mhead {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .mname {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .mname b,
  .mname span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mname b {
    font-weight: 600;
  }
  .nicon {
    width: 40px;
    height: 40px;
    border-radius: 12px;
    display: grid;
    place-items: center;
    background: #111215;
    border: 1px solid #2a2c33;
  }
  .on-dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #6cc59a;
    animation: pl 2s ease-in-out infinite;
  }
  .on-dot.off {
    background: #4a4d55;
    animation: none;
  }
  .usage {
    display: grid;
    grid-template-columns: 44px minmax(0, 1fr) 92px;
    gap: 8px 10px;
    align-items: center;
    font-size: 12px;
  }
  .num {
    text-align: right;
  }
  .spark {
    margin: 4px -16px 0;
  }
  .page :global(.card.link) {
    display: flex;
    align-items: center;
    gap: 18px;
    padding: 18px 22px;
  }
  .node {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
  }
  .ndot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
  }
  .nt {
    font-size: 13px;
    max-width: 22ch;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .wire {
    flex: 1;
    min-width: 0;
    height: 40px;
  }
  .flow {
    stroke-dasharray: 2 10;
    stroke-linecap: round;
    animation: fl 1s linear infinite;
  }
  .flow.r {
    animation-direction: reverse;
    animation-duration: 1.3s;
  }
  .sep {
    width: 1px;
    align-self: stretch;
    background: #25272d;
  }
  .stats {
    display: flex;
    gap: 18px;
    flex: none;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: none;
  }
  .w130 {
    min-width: 130px;
  }
  .w110 {
    min-width: 110px;
  }
  .st {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 13px;
    font-weight: 600;
  }
  .ss {
    font-size: 11.5px;
  }
  .bottom {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    flex: 1;
  }
  .bottom :global(.card.desks) {
    /* Like the mockup: the 440 px base is the content's, without padding or border. */
    box-sizing: content-box;
    flex: 1.3 1 440px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .bottom :global(.card.scenes) {
    box-sizing: content-box;
    flex: 1 1 340px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .ch {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .ch.g4 {
    gap: 4px;
  }
  .ib {
    width: 32px;
    height: 32px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    color: #9a9da6;
    text-decoration: none;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .ib:hover {
    background: #1f2026;
    color: #e8e8eb;
  }
  .ib:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .canvas {
    flex: 1;
    min-height: 230px;
    border-radius: 10px;
    background-color: #0b0c0e;
    background-image: radial-gradient(#17181c 1px, transparent 1px);
    background-size: 18px 18px;
    border: 1px solid #1f2126;
    overflow: hidden;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 8px;
  }
  .scale {
    position: relative;
    width: 100%;
  }
  .desk {
    position: absolute;
    box-sizing: border-box;
    border-radius: 8px;
    padding: 5px;
    background: #0b0c0e;
    border: 1px solid #2f3239;
  }
  .desk.dim {
    border-style: dashed;
    opacity: 0.6;
  }
  .mstate {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    margin-right: 8px;
    font-size: 12px;
    color: #d9a75a;
  }
  .desk .w {
    width: 100%;
    height: 100%;
    border-radius: 4px;
    position: relative;
    display: flex;
    align-items: flex-end;
    padding: 8px 10px;
    box-sizing: border-box;
    font-size: 11.5px;
    color: #d4d6dc;
  }
  .foot {
    position: absolute;
    background: #1c1d22;
  }
  /* The model stands (T-356): the ultrawide G5's V legs and the 32" G5's trapezoid. */
  .foot.v {
    clip-path: polygon(0 100%, 14% 0, 86% 0, 100% 100%, 60% 100%, 50% 40%, 40% 100%);
  }
  .foot.trapezoid {
    clip-path: polygon(16% 0, 84% 0, 100% 100%, 0 100%);
  }
  .wl {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .curdot {
    position: absolute;
    width: 10px;
    height: 10px;
    margin: -5px 0 0 -5px;
    border-radius: 50%;
    background: #e8e8eb;
    box-shadow:
      0 0 0 4px rgba(232, 232, 235, 0.14),
      0 0 18px rgba(232, 232, 235, 0.35);
    transition:
      left 70ms linear,
      top 70ms linear;
    z-index: 2;
  }
  .edge {
    position: absolute;
    border-radius: 2px;
    background: #e8e8eb;
    opacity: 0;
    z-index: 1;
  }
  /* Flash on crossing the edge (the mockup's `eg`, once per crossing). */
  .edge.on {
    animation: eg 1s ease-in-out;
  }
  .slist {
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .scene {
    font: inherit;
    text-align: left;
    cursor: pointer;
    background: #111215;
    border: 1px solid #25272d;
    border-radius: 12px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    color: #e8e8eb;
    min-height: 44px;
    transition:
      border-color 0.25s,
      background 0.25s,
      transform 0.2s;
  }
  .scene:hover {
    border-color: #4a4e58;
  }
  .scene:active {
    transform: scale(0.98);
  }
  .scene.act {
    border-color: #7aa7e6;
    background: #16202e;
  }
  .scene:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 12px;
    width: 100%;
  }
  .thumb {
    width: 84px;
    flex: none;
    height: 46px;
    border-radius: 6px;
    padding: 3px;
    background: #0b0c0e;
    border: 1px solid #2a2c33;
    box-sizing: border-box;
  }
  .thumb div {
    width: 100%;
    height: 100%;
    border-radius: 3px;
    transition: background 0.5s;
  }
  .stext {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
    flex: 1;
  }
  .stext b {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* One line per monitor; a long machine name ends in an ellipsis (the full text is the title). */
  .sd {
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .skey {
    flex: none;
    display: flex;
  }
  @keyframes eg {
    0% {
      opacity: 0;
    }
    18% {
      opacity: 0.9;
    }
    100% {
      opacity: 0;
    }
  }
  .mslot {
    position: relative;
    display: grid;
  }
  .pwr {
    position: absolute;
    top: 6px;
    right: 38px;
  }
</style>
