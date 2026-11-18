<script lang="ts">
  // Puesto (inicio), base de T-112 sobre `V3-Puesto`: máquinas, enlace y escenas con los
  // componentes del sistema. T-113 la completa (escritorios en vivo a escala real, enlace
  // por la IPC).
  import { useStore } from "../lib/context";
  import type { Machine } from "../lib/model";
  import { LINK_NAME } from "../lib/machines";
  import PageHeader from "../shell/PageHeader.svelte";
  import Bar from "../ui/Bar.svelte";
  import Card from "../ui/Card.svelte";
  import Icon from "../ui/Icon.svelte";
  import { machineIcon } from "../ui/icons";
  import Kbd from "../ui/Kbd.svelte";
  import Sparkline from "../ui/Sparkline.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const list = $derived((["a", "b"] as Machine[]).map((id) => desk.machines[id]).filter((m) => m !== null));
  const a = $derived(desk.machines.a);
  const b = $derived(desk.machines.b);
  const link = $derived(desk.link);
  const linkName = $derived(link.kind ? LINK_NAME[link.kind] : "Sin enlace");
  const AMBER = "#d9a75a";
  const WALL = (c: string) => (c === "#7aa7e6" ? "linear-gradient(160deg,#34496a,#1d2738)" : "linear-gradient(160deg,#463d63,#241f33)");
</script>

<div class="page">
  <PageHeader title="Puesto" />

  <section aria-label="Máquinas" class="machines">
    {#each list as m, i (m.id)}
      <Card href="#/maquina/{m.id}" delay={i * 0.06} class="mcard">
        <div class="mhead">
          <span class="nicon"><Icon name={machineIcon(m.os)} size={20} color={m.color} /></span>
          <div class="mname"><b>{m.name}</b><span class="mut sm">{m.detail}</span></div>
          <span class="fill"></span><span class="on-dot" class:off={m.power !== "on"} title={m.power === "on" ? "Encendida" : "Apagada"}></span>
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
    {/each}
  </section>

  {#if a && b}
    <Card href="#/red" delay={0.16} class="link" label="Enlace entre {a.name} y {b.name}: {linkName}">
      <span class="node"><span class="ndot" style="background:{a.color}"></span><span class="nt">{a.name}</span></span>
      <svg viewBox="0 0 600 40" preserveAspectRatio="none" class="wire" aria-hidden="true">
        <defs><linearGradient id="lk" x1="0" x2="1"><stop offset="0" stop-color={a.color}></stop><stop offset="1" stop-color={b.color}></stop></linearGradient></defs>
        <line x1="0" y1="14" x2="600" y2="14" stroke="#24262c" stroke-width="2"></line>
        {#if link.kind}<line class="flow" x1="0" y1="14" x2="600" y2="14" stroke="url(#lk)" stroke-width="3"></line>{/if}
        <line x1="0" y1="30" x2="600" y2="30" stroke="#24262c" stroke-width="1.5" stroke-dasharray="3 6"></line>
        {#if link.kind}<line class="flow r" x1="0" y1="30" x2="600" y2="30" stroke={b.color} stroke-opacity=".35" stroke-width="1.5"></line>{/if}
      </svg>
      <span class="node"><span class="nt">{b.name}</span><span class="ndot" style="background:{b.color}"></span></span>
      <span class="sep"></span>
      <span class="stat w130">
        <span class="st"><Icon name={link.kind === "wifi" ? "wifi" : "bolt"} size={13} color={a.color} />{linkName}</span>
        <span class="mono mut ss">{link.gbps !== null ? `${link.gbps} Gbps` : "—"} · {link.ms !== null ? `${link.ms} ms` : "—"}</span>
      </span>
      <span class="stat w110">
        <span class="st">Entrada</span>
        <span class="mono mut ss">{link.inputP99 !== null ? `p99 ${link.inputP99} ms` : "—"}</span>
      </span>
    </Card>
  {/if}

  <section class="bottom">
    <Card delay={0.2} class="desks">
      <div class="ch"><h2 class="h3">Escritorios en vivo</h2><span class="fill"></span><a href="#/acomodo" class="ib" aria-label="Editar acomodo" title="Editar acomodo"><Icon name="edit" /></a></div>
      <div class="canvas">
        {#if a && b}
          <div role="img" aria-label="{a.name} a la izquierda de {b.name}" class="scale">
            <div class="desk" style="left:7.5%;top:36%;width:22.9%;height:35.8%"><div class="w" style="background:{WALL(a.color)}">{a.name}</div></div>
            <div class="foot" style="left:6%;top:71.8%;width:25.9%;height:2.6%;border-radius:0 0 6px 6px"></div>
            <div class="desk" style="left:32.6%;top:10.9%;width:60%;height:60.9%"><div class="w" style="background:{WALL(b.color)}">34" · {b.name}</div></div>
            <div class="foot" style="left:60.4%;top:71.8%;width:4.5%;height:9%"></div>
            <div class="foot" style="left:55.1%;top:80.8%;width:15%;height:2.6%;border-radius:3px"></div>
            <span class="edge"></span>
            <span class="curdot"></span>
          </div>
        {/if}
      </div>
    </Card>

    <Card delay={0.24} class="scenes">
      <div class="ch g4">
        <h2 class="h3">Escenas</h2><span class="fill"></span>
        <a href="#/monitores" class="ib" aria-label="Nueva escena" title="Nueva escena"><Icon name="plus" /></a>
        <a href="#/monitores" class="ib" aria-label="Editar escenas" title="Editar escenas"><Icon name="edit" /></a>
      </div>
      <div class="slist">
        {#each desk.scenes as s (s.id)}
          {@const on = desk.activeScene === s.id}
          {@const shows = s.shows ? desk.machines[s.shows] : null}
          <button type="button" class="scene" class:act={on} aria-pressed={on} onclick={() => store.applyScene(s.id)}>
            <span class="srow">
              <span class="thumb"><div style="background:{WALL(shows?.color ?? '#7aa7e6')}"></div></span>
              <span class="stext"><b>{s.name}</b><span class="mut sd">{s.desc}</span></span>
              <Kbd>{s.key}</Kbd>
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
  .machines {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 14px;
  }
  .machines :global(.card.mcard) {
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
    flex: 1.3 1 440px;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .bottom :global(.card.scenes) {
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
    aspect-ratio: 133/55;
  }
  .desk {
    position: absolute;
    box-sizing: border-box;
    border-radius: 8px;
    padding: 5px;
    background: #0b0c0e;
    border: 1px solid #2f3239;
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
  .curdot {
    position: absolute;
    left: 16%;
    top: 56%;
    width: 10px;
    height: 10px;
    margin: -5px 0 0 -5px;
    border-radius: 50%;
    background: #e8e8eb;
    box-shadow:
      0 0 0 4px rgba(232, 232, 235, 0.14),
      0 0 18px rgba(232, 232, 235, 0.35);
    animation: cur 7s cubic-bezier(0.45, 0, 0.3, 1) infinite;
    z-index: 2;
  }
  .edge {
    position: absolute;
    left: calc(32.6% - 1px);
    top: 36%;
    width: 2px;
    height: 35.8%;
    border-radius: 2px;
    background: #e8e8eb;
    opacity: 0;
    animation: eg 7s ease-in-out infinite;
    z-index: 1;
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
  }
  .sd {
    font-size: 12px;
    white-space: nowrap;
  }
  @keyframes cur {
    0% {
      left: 16%;
      top: 56%;
    }
    22% {
      left: 25%;
      top: 52%;
    }
    38% {
      left: 40%;
      top: 48%;
    }
    62% {
      left: 63%;
      top: 30%;
    }
    80% {
      left: 80%;
      top: 55%;
    }
    100% {
      left: 16%;
      top: 56%;
    }
  }
  @keyframes eg {
    0%,
    30% {
      opacity: 0;
    }
    34% {
      opacity: 0.9;
    }
    48% {
      opacity: 0;
    }
    100% {
      opacity: 0;
    }
  }
</style>
