<script lang="ts">
  // Tunnels (`V3-Compute`, solo lectura por D9): proyectos plegables to the left of, el detalle
  // del elegido to the right of con el tubo, las cifras y el panel de procesos con su salida en vivo.
  import { onMount } from "svelte";
    import { useStore } from "../lib/context";
  import { DEFAULT_COLOR } from "../lib/machines";
  import type { Machine } from "../lib/model";
  import type { ProcessRow, TunnelView } from "../lib/store/types";
  import { EMPTY_OUTPUT, appendOutput, bytesLabel, elapsed, emptyLabel, groupByProject, latencyLabel, pickProcess, pickTunnel, processesLabel, ratio, runningOf, shortLabels, statusText, summary, tubeSpeed, type Output } from "../lib/tunnels";
  import Card from "../ui/Card.svelte";

  const store = useStore();
  const desk = $derived(store.desk);

  /** El comando del servidor MCP con el que Claude abre los tunnels (DESIGN §8). */
  const MCP_COMMAND = "tandem mcp";

  let wanted: string | null = $state(null);
  const groups = $derived(groupByProject(desk.tunnels));
  const sel = $derived(pickTunnel(desk.tunnels, wanted));
  /** Proyectos plegados por nombre. */
  let folded: Record<string, boolean> = $state({});
  /** El proceso elegido en cada tunnel. */
  let chosen: Record<string, number> = $state({});
  let now = $state(store.now());

  let rows: ProcessRow[] = $state([]);
  const running = $derived(runningOf(rows));
  const proc = $derived(sel ? pickProcess(running, chosen[sel.id] ?? null) : null);
  let output: Output = $state(EMPTY_OUTPUT);
  let body: HTMLElement | undefined = $state();

  const machine = (id: Machine) => desk.machines[id];
  const color = (id: Machine) => machine(id)?.color ?? DEFAULT_COLOR[id];
  const nameOf = (id: Machine) => machine(id)?.name ?? id.toUpperCase();
  /** Los nombres cortos del camino de un tunnel (`Mac → PC`). */
  const ends = (t: TunnelView): [string, string] => {
    const a = machine(t.from);
    const b = machine(t.to);
    return a && b ? shortLabels(a, b) : [nameOf(t.from), nameOf(t.to)];
  };
  /** Dónde corren los procesos: lo que dice el agente, o el sistema de la machine destino. */
  const envOf = (t: TunnelView) => t.env ?? machine(t.to)?.detail ?? "";

  const closed = $derived(sel?.state === "closed");
  const stateText = $derived(sel ? statusText(sel, now) : "");
  const speed = $derived(sel ? tubeSpeed(sel.state) : null);

  const chipBorder = { busy: "#4f7cbc", idle: "#31343b", closed: "#31343b" } as const;
  const chipColor = { busy: "#c3d8f6", idle: "#9a9da6", closed: "#6a6d76" } as const;

  // El reloj de las horas («34 m») avanza solo; no es una animación.
  onMount(() => {
    const t = setInterval(() => (now = store.now()), 1000);
    return () => clearInterval(t);
  });

  // La lista de procesos del tunnel elegido, al elegirlo y cada segundo y medio.
  $effect(() => {
    const id = sel?.id;
    const state = sel?.state;
    rows = [];
    if (!id || state === "closed") return;
    let alive = true;
    const load = async () => {
      try {
        const r = await store.processes(id);
        if (alive) rows = r;
      } catch {
        // Sin agente o sin ese tunnel: la lista queda como estaba y el próximo intento lo reintenta.
      }
    };
    void load();
    const t = setInterval(load, 1500);
    return () => {
      alive = false;
      clearInterval(t);
    };
  });

  // La salida del proceso elegido desde el principio, y después lo fresh cada segundo.
  $effect(() => {
    const id = sel?.id;
    const pid = proc?.id;
    output = EMPTY_OUTPUT;
    if (!id || pid === undefined) return;
    let alive = true;
    let cursor = EMPTY_OUTPUT.cursor;
    const pull = async () => {
      try {
        const chunk = await store.output(id, pid, cursor);
        if (!alive) return;
        cursor = chunk.next;
        if (chunk.text !== "") output = appendOutput(output, chunk);
      } catch {
        // Igual que la lista: se reintenta en el próximo ciclo.
      }
    };
    void pull();
    const t = setInterval(pull, 1000);
    return () => {
      alive = false;
      clearInterval(t);
    };
  });

  // Si el que mira estaba al final, la salida nueva lo sigue.
  $effect(() => {
    void output.text;
    if (body && body.scrollHeight - body.scrollTop - body.clientHeight < 40) body.scrollTop = body.scrollHeight;
  });

  const pick = (t: TunnelView) => (wanted = t.id);
  const toggle = (name: string) => (folded = { ...folded, [name]: !folded[name] });

  async function copyCommand() {
    try {
      await navigator.clipboard.writeText(MCP_COMMAND);
      store.toast(`Copied: ${MCP_COMMAND}`, "ok");
    } catch {
      store.toast("Could not copy the command", "bad");
    }
  }
</script>

<div class="page">
  <header class="head">
    <h1>Tunnels</h1>
    <span class="chip"><span class="pdot"></span>{summary(groups)}</span>
    <span class="fill"></span>
    <button type="button" class="chip act" onclick={copyCommand} title="Copy the command that connects Claude">
      <svg viewBox="0 0 24 24" fill="#cbbff5" aria-hidden="true"><path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8z"></path></svg>How Claude opens them
    </button>
  </header>

  {#if sel}
    <div class="cols">
      <Card tag="section" label="Tunnels by project" style="box-sizing:content-box;flex:1 1 300px;max-width:380px;min-width:0;--card-pad:8px;display:flex;flex-direction:column;gap:2px">
        {#each groups as g, gi (g.name)}
          {@const open = !folded[g.name]}
          <!-- Plegar vuelve a dibujar el encabezado, como el mockup: el cambio entra otra vez. -->
          {#key open}
          <button type="button" class="proj" style="animation-delay:{gi * 0.1}s" onclick={() => toggle(g.name)} aria-expanded={open}>
            <span class="pf"><svg viewBox="0 0 24 24" fill="none" stroke="#c4c6cc" stroke-width="1.8" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path></svg></span>
            <span class="pn"><b>{g.name}</b>{#if g.path}<span class="mut mono pp">{g.path}</span>{/if}</span>
            <span class="cnt" title="{g.live} open of {g.tunnels.length}"><span class="cd" class:live={g.live > 0}></span><span class="mono">{ratio(g)}</span></span>
            <svg class="chev" class:folded={!open} width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6"></path></svg>
          </button>
          {/key}
          {#if open}
            <div class="pt">
              {#each g.tunnels as t, k (t.id)}
                {@const on = t.id === sel.id}
                {@const sp = tubeSpeed(t.state)}
                <button type="button" class="tun" class:on class:closed={t.state === "closed"} style="animation-delay:{(gi * 3 + k + 1) * 0.05}s" onclick={() => pick(t)} aria-pressed={on}>
                  <span class="row1"><svg width="13" height="13" viewBox="0 0 24 24" fill={t.state === "closed" ? "#4a4d55" : "#cbbff5"} aria-hidden="true"><path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8z"></path></svg><b>{t.title}</b><span class="mono st {t.state}">{statusText(t, now)}</span></span>
                  {#if t.branch}
                    <span class="row2"><span class="br"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" aria-hidden="true"><circle cx="6" cy="6" r="2.5"></circle><circle cx="6" cy="18" r="2.5"></circle><circle cx="18" cy="8" r="2.5"></circle><path d="M6 8.5v7M18 10.5c0 4-6 3-11 5.5"></path></svg>{t.branch}</span></span>
                  {/if}
                  <span class="mini" style="--c1:{t.state === 'closed' ? '#3a3d45' : color(t.from)};--sp:{sp ? `${sp.a}s` : '0s'}{t.state === 'closed' ? ';opacity:.5' : ''}">
                    <span class="dot" style="background:{color(t.from)}"></span><span class="mut">{ends(t)[0]}</span><span class="ln"><span></span></span><span class="mut">{ends(t)[1]}</span><span class="dot" style="background:{color(t.to)}"></span>
                  </span>
                </button>
              {/each}
            </div>
          {/if}
        {/each}
      </Card>

      <Card tag="section" label="Tunnel detail" style="box-sizing:content-box;flex:999 1 500px;min-width:0;display:flex;flex-direction:column;gap:16px">
        <div class="dh">
          <div class="dt">
            <span class="mut crumb">
              <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"></path></svg>{sel.project}
              {#if sel.branch}
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" aria-hidden="true"><path d="m9 6 6 6-6 6"></path></svg>
                <span class="br"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" aria-hidden="true"><circle cx="6" cy="6" r="2.5"></circle><circle cx="6" cy="18" r="2.5"></circle><circle cx="18" cy="8" r="2.5"></circle><path d="M6 8.5v7M18 10.5c0 4-6 3-11 5.5"></path></svg>{sel.branch}</span>
              {/if}
            </span>
            <h2><svg width="15" height="15" viewBox="0 0 24 24" fill="#cbbff5" aria-hidden="true"><path d="M12 2l1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8z"></path></svg>{sel.title}</h2>
          </div>
          <span class="fill"></span>
          <span class="chip" style="border-color:{chipBorder[sel.state]};color:{chipColor[sel.state]}">{stateText}</span>
        </div>

        <div class="flow" role="img" aria-label="Tunnel {sel.title} from {nameOf(sel.from)} to {nameOf(sel.to)}">
          <div class="node" style="--c:{color(sel.from)}">
            <span class="nic"><svg viewBox="0 0 24 24" fill="none" stroke={color(sel.from)} stroke-width="1.6" aria-hidden="true"><path d={machine(sel.from)?.os === "macos" ? "M4 5h16v11H4zM2 19h20" : "M6 3h12v18H6zM9 7h6M12 13a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z"}></path></svg></span>
            <b>{nameOf(sel.from)}</b><span class="mut sm">Claude session</span>
          </div>
          <div class="tube">
            <svg width="100%" height="96" viewBox="0 0 400 96" preserveAspectRatio="none" aria-hidden="true">
              <defs><linearGradient id="tu-tg" x1="0" x2="1"><stop offset="0" stop-color={color(sel.from)}></stop><stop offset="1" stop-color={color(sel.to)}></stop></linearGradient></defs>
              <rect x="0" y="30" width="400" height="36" rx="18" fill="#111215" stroke="#25272d"></rect>
              <line class="flowA" class:off={!speed} x1="10" y1="42" x2="390" y2="42" stroke="url(#tu-tg)" stroke-width="3" style={speed ? `animation-duration:${speed.a}s` : ""}></line>
              <line class="flowB" class:off={!speed} x1="10" y1="54" x2="390" y2="54" stroke="url(#tu-tg)" stroke-opacity=".5" stroke-width="2" style={speed ? `animation-duration:${speed.b}s` : ""}></line>
            </svg>
            <span class="lbl" style="top:0"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#7aa7e6" stroke-width="2.4" aria-hidden="true"><path d="M5 12h14M13 6l6 6-6 6"></path></svg><span class="mono">{bytesLabel(sel.traffic.up)}</span></span>
            <span class="lbl" style="bottom:0"><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#a99be8" stroke-width="2.4" aria-hidden="true"><path d="M19 12H5M11 6l-6 6 6 6"></path></svg><span class="mono">{bytesLabel(sel.traffic.down)}</span></span>
          </div>
          <div class="node" style="--c:{color(sel.to)}">
            <span class="nic"><svg viewBox="0 0 24 24" fill="none" stroke={color(sel.to)} stroke-width="1.6" aria-hidden="true"><path d={machine(sel.to)?.os === "macos" ? "M4 5h16v11H4zM2 19h20" : "M6 3h12v18H6zM9 7h6M12 13a2.5 2.5 0 1 0 0 5 2.5 2.5 0 0 0 0-5z"}></path></svg></span>
            <b>{nameOf(sel.to)}</b><span class="mut sm">{envOf(sel)}</span>
          </div>
        </div>

        <div class="stats">
          <div class="stat"><span class="mut sm">Opened</span><b class="mono">{closed ? "—" : elapsed(now - sel.created)}</b></div>
          <div class="stat"><span class="mut sm">Commands</span><b class="mono">{sel.commands ?? rows.length}</b></div>
          <div class="stat"><span class="mut sm">Live processes</span><b class="mono">{sel.processes}</b></div>
          {#if sel.ports !== null}<div class="stat"><span class="mut sm">Ports</span><b class="mono">{sel.ports}</b></div>{/if}
          <div class="stat"><span class="mut sm">Latency</span><b class="mono">{closed ? "—" : latencyLabel(desk.link.ms)}</b></div>
        </div>

        <div class="below">
          <div class="pwrap">
            {#if running.length > 0 && proc}
              <div class="plist" role="listbox" aria-label="Running processes">
                <div class="plh"><span class="spin" style="width:9px;height:9px"></span>{processesLabel(running.length)}</div>
                {#each running as q, i (q.id)}
                  <button type="button" role="option" class="prow" aria-selected={q.id === proc.id} onclick={() => (chosen[sel.id] = q.id)} style="animation-delay:{i * 0.04}s"><span class="spin"></span><span class="mono pc">{q.command}</span><span class="mono mut since">{elapsed(now - q.startedAt)}</span></button>
                {/each}
              </div>
              <div class="pout">
                <div class="poh"><svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="#9a9da6" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="m5 8 4 4-4 4M12 16h7"></path></svg><span class="mono cmd">{proc.command}</span><span class="mono mut since">{proc.pid === null ? "" : `pid ${proc.pid} · `}{elapsed(now - proc.startedAt)}</span></div>
                <div class="mono pbody" bind:this={body}>{output.text}<span class="caret"></span></div>
              </div>
            {:else}
              <div class="pempty">
                <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#4a4d55" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M3 12h4M17 12h4"></path><rect x="7" y="8" width="10" height="8" rx="4"></rect></svg><span>{emptyLabel(sel)}</span>
              </div>
            {/if}
          </div>
        </div>
      </Card>
    </div>
  {:else}
    <Card tag="section" label="Tunnels by project" style="flex:1;display:flex">
      <div class="pempty none">
        <svg width="28" height="28" viewBox="0 0 24 24" fill="none" stroke="#4a4d55" stroke-width="1.6" stroke-linecap="round" aria-hidden="true"><path d="M3 12h4M17 12h4"></path><rect x="7" y="8" width="10" height="8" rx="4"></rect></svg><span>No open tunnels</span>
      </div>
    </Card>
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .fill {
    flex: 1;
  }
  .mut {
    color: #9a9da6;
  }
  .mono {
    font-family: "JetBrains Mono", ui-monospace, monospace;
  }
  .sm {
    font-size: 11.5px;
  }

  /* Chips de la cabecera y del detalle (30 px, como `.chip` del mockup). */
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 30px;
    padding: 0 12px;
    border-radius: 999px;
    border: 1px solid #31343b;
    font: inherit;
    font-size: 12.5px;
    color: #c4c6cc;
    background: transparent;
    white-space: nowrap;
    box-sizing: content-box;
    transition:
      border-color 0.2s,
      background 0.2s,
      transform 0.2s;
  }
  .chip svg {
    width: 14px;
    height: 14px;
    flex: none;
  }
  button.chip {
    cursor: pointer;
  }
  .chip.act:hover {
    border-color: #4a4e58;
    background: #17181c;
    transform: translateY(-1px);
  }
  .chip.act:focus-visible,
  .proj:focus-visible,
  .tun:focus-visible,
  .prow:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .pdot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #a99be8;
    animation: tu-pl 1.8s infinite;
  }
  .br {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 20px;
    padding: 0 7px;
    border-radius: 6px;
    background: #1b1c21;
    border: 1px solid #2a2c33;
    font-family: "JetBrains Mono", ui-monospace, monospace;
    font-size: 10.5px;
    color: #9a9da6;
    white-space: nowrap;
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .br svg {
    width: 10px;
    height: 10px;
    flex: none;
  }

  .cols {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    align-items: stretch;
    flex: 1;
  }

  /* Proyectos y tunnels. */
  .proj {
    font: inherit;
    color: #e8e8eb;
    text-align: left;
    width: 100%;
    cursor: pointer;
    background: transparent;
    border: 0;
    border-radius: 11px;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px;
    margin-top: 4px;
    animation: tu-up 0.35s ease-out both;
    transition: background 0.2s;
    box-sizing: border-box;
  }
  .proj:hover {
    background: #18191d;
  }
  .proj:hover .pf {
    transform: rotate(-6deg) scale(1.06);
    border-color: #3a3d45;
  }
  .pf {
    width: 32px;
    height: 32px;
    border-radius: 9px;
    display: grid;
    place-items: center;
    background: #1b1c21;
    border: 1px solid #2a2c33;
    flex: none;
    transition:
      transform 0.3s cubic-bezier(0.3, 1.5, 0.5, 1),
      border-color 0.2s;
  }
  .pf svg {
    width: 16px;
    height: 16px;
  }
  .pn {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .pn b {
    font-weight: 600;
    font-size: 13.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pp {
    font-size: 10.5px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cnt {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    padding: 0 9px;
    border-radius: 999px;
    border: 1px solid #2e3138;
    background: #111215;
    font-size: 11.5px;
    color: #c4c6cc;
    flex: none;
  }
  .cd {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: #4a4d55;
  }
  .cd.live {
    background: #a99be8;
    animation: tu-pl 1.8s infinite;
  }
  .chev {
    color: #80838d;
    flex: none;
    transition: transform 0.3s cubic-bezier(0.3, 1.4, 0.5, 1);
  }
  .chev.folded {
    transform: rotate(-90deg);
  }
  .pt {
    display: flex;
    flex-direction: column;
    gap: 2px;
    margin-left: 16px;
    padding-left: 6px;
    border-left: 1px solid #1f2126;
    animation: tu-dn 0.3s ease-out;
  }
  .tun {
    font: inherit;
    text-align: left;
    width: 100%;
    cursor: pointer;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 12px;
    padding: 10px 12px 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 7px;
    color: #e8e8eb;
    transition:
      background 0.2s,
      border-color 0.2s,
      transform 0.2s;
    animation: tu-up 0.35s ease-out both;
    position: relative;
    box-sizing: border-box;
  }
  .tun::before {
    content: "";
    position: absolute;
    left: 4px;
    top: 12px;
    bottom: 12px;
    width: 2px;
    border-radius: 1px;
    background: #25272d;
  }
  .tun:hover {
    background: #18191d;
    transform: translateX(2px);
  }
  .tun.on {
    background: #1c1d22;
    border-color: #31343b;
  }
  .tun.closed {
    opacity: 0.6;
  }
  .row1 {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
  }
  .row1 b {
    font-weight: 600;
    font-size: 13.5px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    flex: 1;
    min-width: 0;
  }
  .st {
    font-size: 11px;
    flex: none;
  }
  .st.busy {
    color: #c3d8f6;
  }
  .st.idle {
    color: #9a9da6;
  }
  .st.closed {
    color: #6a6d76;
  }
  .row2 {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
  }
  .mini {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11.5px;
  }
  .mini .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    flex: none;
  }
  .mini .ln {
    flex: 1;
    height: 2px;
    border-radius: 1px;
    position: relative;
    overflow: hidden;
    background: #25272d;
    min-width: 24px;
  }
  .mini .ln span {
    position: absolute;
    inset: 0;
    background: repeating-linear-gradient(90deg, var(--c1) 0 6px, transparent 6px 14px);
    animation: tu-mv var(--sp) linear infinite;
  }

  /* Detalle. */
  .dh {
    display: flex;
    align-items: flex-start;
    gap: 10px;
  }
  .dt {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }
  .crumb {
    font-size: 12px;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  h2 {
    margin: 0;
    font-size: 17px;
    font-weight: 650;
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .flow {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    flex: none;
    width: 130px;
    text-align: center;
  }
  .node b {
    font-size: 13px;
    font-weight: 600;
  }
  .nic {
    width: 58px;
    height: 58px;
    border-radius: 16px;
    display: grid;
    place-items: center;
    background: #111215;
    border: 1px solid #2a2c33;
    position: relative;
  }
  .nic::after {
    content: "";
    position: absolute;
    inset: -5px;
    border-radius: 20px;
    border: 1px solid var(--c);
    opacity: 0.35;
    animation: tu-halo 2.4s ease-in-out infinite;
  }
  .nic svg {
    width: 26px;
    height: 26px;
  }
  .tube {
    flex: 1;
    min-width: 0;
    position: relative;
    height: 96px;
  }
  .flowA {
    stroke-dasharray: 3 13;
    stroke-linecap: round;
    animation: tu-fa 0.9s linear infinite;
  }
  .flowB {
    stroke-dasharray: 3 17;
    stroke-linecap: round;
    animation: tu-fb 1.6s linear infinite;
  }
  .flowA.off,
  .flowB.off {
    animation: none;
    opacity: 0.2;
  }
  .lbl {
    position: absolute;
    left: 50%;
    transform: translateX(-50%);
    font-size: 11.5px;
    white-space: nowrap;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 3px 9px;
    border-radius: 999px;
    background: #0e0f12;
    border: 1px solid #25272d;
  }
  .stats {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  .stat {
    display: flex;
    flex-direction: column;
    gap: 3px;
    padding: 10px 12px;
    border-radius: 10px;
    background: #111215;
    border: 1px solid #1f2126;
    flex: 1 1 110px;
    min-width: 0;
    transition: border-color 0.2s;
  }
  .stat:hover {
    border-color: #3a3d45;
  }
  .stat b {
    font-size: 17px;
    font-weight: 500;
  }

  /* Procesos y salida. */
  .below {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    align-items: flex-start;
  }
  .pwrap {
    flex: 1 1 100%;
    display: grid;
    grid-template-columns: minmax(220px, 300px) minmax(0, 1fr);
    height: 290px;
    border: 1px solid #25272d;
    border-radius: 12px;
    overflow: hidden;
    background: #0f1013;
  }
  .plist {
    border-right: 1px solid #1f2126;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 0 6px 6px;
  }
  .plh {
    position: sticky;
    top: 0;
    background: #0f1013;
    font-size: 11.5px;
    color: #9a9da6;
    padding: 10px 8px 8px;
    display: flex;
    gap: 8px;
    align-items: center;
    z-index: 1;
  }
  .prow {
    font: inherit;
    color: #c4c6cc;
    background: transparent;
    border: 0;
    border-radius: 8px;
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 8px 10px 8px 12px;
    cursor: pointer;
    text-align: left;
    position: relative;
    flex: none;
    transition:
      background 0.2s,
      color 0.2s;
    animation: tu-up 0.3s ease-out both;
  }
  .prow:hover {
    background: #18191d;
    color: #e8e8eb;
  }
  .prow[aria-selected="true"] {
    background: #1c1d22;
    color: #e8e8eb;
  }
  .prow[aria-selected="true"]::before {
    content: "";
    position: absolute;
    left: 2px;
    top: 9px;
    bottom: 9px;
    width: 3px;
    border-radius: 2px;
    background: #7aa7e6;
    animation: tu-bar 0.25s cubic-bezier(0.3, 1.4, 0.5, 1);
  }
  .pc {
    font-size: 12px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .since {
    font-size: 11px;
    flex: none;
  }
  .spin {
    width: 13px;
    height: 13px;
    border-radius: 50%;
    border: 2px solid #2a3a52;
    border-top-color: #7aa7e6;
    animation: tu-sp 0.9s linear infinite;
    box-sizing: content-box;
    flex: none;
  }
  .pout {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .poh {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 10px 14px;
    border-bottom: 1px solid #1f2126;
    flex: none;
  }
  .cmd {
    font-size: 12.5px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pbody {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 12px 14px;
    font-size: 12px;
    line-height: 1.7;
    color: #c4c6cc;
    white-space: pre-wrap;
    background: #0a0b0d;
  }
  .caret {
    display: inline-block;
    width: 7px;
    height: 14px;
    background: #c4c6cc;
    vertical-align: -2px;
    animation: tu-bl 1s steps(1) infinite;
  }
  .pempty {
    grid-column: 1 / -1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 10px;
    color: #80838d;
    font-size: 13px;
  }
  .pempty.none {
    flex: 1;
    min-height: 240px;
  }

  @keyframes tu-up {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes tu-dn {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes tu-bar {
    from {
      transform: scaleY(0);
    }
    to {
      transform: scaleY(1);
    }
  }
  @keyframes tu-sp {
    to {
      transform: rotate(360deg);
    }
  }
  @keyframes tu-mv {
    from {
      transform: translateX(-14px);
    }
    to {
      transform: translateX(0);
    }
  }
  @keyframes tu-fa {
    to {
      stroke-dashoffset: -32;
    }
  }
  @keyframes tu-fb {
    to {
      stroke-dashoffset: 40;
    }
  }
  @keyframes tu-halo {
    0%,
    100% {
      transform: scale(1);
      opacity: 0.35;
    }
    50% {
      transform: scale(1.06);
      opacity: 0.08;
    }
  }
  @keyframes tu-pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(169, 155, 232, 0.6);
    }
    50% {
      box-shadow: 0 0 0 5px rgba(169, 155, 232, 0);
    }
  }
  @keyframes tu-bl {
    50% {
      opacity: 0;
    }
  }
</style>
