<script lang="ts" module>
  /** Lo que la bandeja le pide a la app (Tauri, o un doble en demo y pruebas). */
  export interface TrayActions {
    showMain(): void | Promise<void>;
    quit(): void | Promise<void>;
  }
</script>

<script lang="ts">
  // Barra de menú / bandeja (`V3-Tray`, 380×600): las dos máquinas, escenas, túneles y
  // avisos, con «Abrir» y «Salir». «Lock here» no está hasta que la IPC tenga la acción.
  import { onDestroy, onMount } from "svelte";
  import { shortName } from "./lib/commands";
  import { machineLine, shortLabels } from "./lib/tunnels";
  import { LINK_NAME } from "./lib/machines";
  import type { Machine } from "./lib/model";
  import type { Store } from "./lib/store/store.svelte";
  import type { TunnelView } from "./lib/store/types";
  import Logo from "./shell/Logo.svelte";
  import Chip from "./ui/Chip.svelte";
  import Icon from "./ui/Icon.svelte";

  interface Props {
    store: Store;
    actions: TrayActions;
  }
  let { store, actions }: Props = $props();

  onMount(() => {
    void store.start();
  });
  onDestroy(() => store.stop());

  const desk = $derived(store.desk);
  const machines = $derived((["a", "b"] as Machine[]).map((id) => desk.machines[id]).filter((m) => m !== null));
  /** Un renglón por proyecto (como la bandeja del mockup): activo si algún túnel trabaja; los activos primero. */
  const open = $derived.by(() => {
    const byProject = new Map<string, TunnelView>();
    for (const t of desk.tunnels) {
      if (t.state === "closed") continue;
      const seen = byProject.get(t.project);
      if (!seen || (seen.state !== "busy" && t.state === "busy")) byProject.set(t.project, { ...t, id: t.project });
    }
    return [...byProject.values()].sort((x, y) => Number(y.state === "busy") - Number(x.state === "busy") || x.project.localeCompare(y.project));
  });
  const name = (id: Machine) => shortName(desk.machines[id]?.name ?? id.toUpperCase());
  // Los mismos rótulos que la lista de Túneles: «Mac → PC» si son de sistemas distintos.
  const path = (from: Machine, to: Machine): string => {
    const a = desk.machines[from];
    const b = desk.machines[to];
    const [x, y] = a && b ? shortLabels(a, b) : [name(from), name(to)];
    return `${x} → ${y}`;
  };
  const link = $derived(desk.link.kind ? `${LINK_NAME[desk.link.kind]}${desk.link.ms !== null ? ` · ${desk.link.ms} ms` : ""}` : "no link");
  const hot = $derived(machines.find((m) => (m.resources?.gpu?.percent ?? 0) >= 85));
  const AMBER = "#d9a75a";
  /** Estado difícil (`V3-States`) en una línea: el mockup de la bandeja solo dibuja el aviso de la VRAM. */
  const peer = $derived<Machine>(desk.here === "a" ? "b" : "a");
  const canWake = $derived(desk.trouble === "peer-off" && store.can("power"));
</script>

<div class="pop">
  <div class="head"><Logo size={18} stroke={2.6} />My desk<span class="fill"></span><span class="mono mut ln">{link}</span></div>

  {#if desk.locked}<Chip tone="warn" icon="lock">Cursor locked here</Chip>{/if}

  <div class="box">
    {#each machines as m, i (m.id)}
      {@const load = m.resources?.gpu ?? m.resources?.cpu}
      <div class="row" class:sep={i > 0}>
        <span class="dot" style="background:{m.color}"></span><b>{m.name}</b>
        <span class="mut sm">{machineLine(m, desk.tunnels)}</span>
        <span class="fill"></span>
        {#if load}<div class="bar"><span style="width:{load.percent}%;background:{load.percent >= 85 ? AMBER : m.color}"></span></div>{/if}
      </div>
    {/each}
  </div>

  {#if desk.scenes.length > 0}
    <span class="mut cap">Scenes</span>
    <div class="scenes">
      {#each desk.scenes as s (s.id)}
        <button type="button" class="sc" class:act={desk.activeScene === s.id} aria-pressed={desk.activeScene === s.id} disabled={!store.can("scenes")} onclick={() => store.applyScene(s.id)}
          >{s.name}</button
        >
      {/each}
    </div>
  {/if}

  {#if open.length > 0}
    <span class="mut cap">Tunnels</span>
    <div class="box tl">
      {#each open as t, i (t.id)}
        <div class="row" class:sep={i > 0}>
          <span class="dot" style="background:{t.state === 'busy' ? (desk.machines[t.from]?.color ?? '#7aa7e6') : '#4a4d55'}"></span>{t.project}
          <span class="fill"></span><span class="mono mut ss">{path(t.from, t.to)} · {t.state === "busy" ? "activo" : "inactivo"}</span>
        </div>
      {/each}
    </div>
  {/if}

  {#if desk.trouble !== "none"}
    <div class="warn" role="status">
      <Icon name="warn" size={16} stroke={2} /><span class="fill">{desk.troubleText}</span>
      {#if canWake}<button type="button" class="lnk on" onclick={() => store.setPower(peer, true)}>Power on</button>{/if}
    </div>
  {:else if hot?.resources?.gpu}
    <div class="warn"><Icon name="warn" size={16} stroke={2} />{hot.name} with {hot.resources.gpuLabel} at {hot.resources.gpu.percent} %</div>
  {/if}

  <span class="fill"></span>
  <div class="foot">
    <button type="button" class="lnk on" onclick={() => actions.showMain()}><Icon name="window" size={15} />Open</button>
    <span class="fill"></span>
    <button type="button" class="lnk" aria-label="Leave" title="Leave" onclick={() => actions.quit()}><Icon name="power" size={15} stroke={2} /></button>
  </div>
</div>

<style>
  .pop {
    width: 380px;
    height: 600px;
    box-sizing: border-box;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    font-family: "Inter", system-ui, sans-serif;
    color: #e8e8eb;
    background: #15161a;
    border: 1px solid #31343b;
    border-radius: 16px;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 650;
    font-size: 14px;
  }
  .ln {
    font-size: 11.5px;
    font-weight: 400;
  }
  .fill {
    flex: 1;
  }
  .box {
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    background: #111215;
    border: 1px solid #1f2126;
    padding: 6px 12px;
  }
  .box.tl {
    gap: 2px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 13px;
    padding: 8px 2px;
    transition: background 0.2s;
  }
  .row.sep {
    border-top: 1px solid #1f2126;
  }
  .row b {
    font-weight: 600;
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
  }
  .sm {
    font-size: 12px;
  }
  .ss {
    font-size: 11.5px;
  }
  .cap {
    font-size: 11.5px;
    padding-top: 4px;
  }
  .bar {
    height: 5px;
    border-radius: 3px;
    background: #24262c;
    overflow: hidden;
    flex: 1;
    max-width: 70px;
  }
  .bar span {
    display: block;
    height: 100%;
    transition: width 0.6s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .scenes {
    display: flex;
    gap: 6px;
  }
  .sc {
    font: inherit;
    font-size: 12.5px;
    flex: 1;
    padding: 9px 6px;
    border-radius: 9px;
    border: 1px solid #25272d;
    background: #15161a;
    color: #c4c6cc;
    cursor: pointer;
    min-height: 40px;
    transition:
      border-color 0.25s,
      background 0.25s,
      transform 0.2s;
  }
  .sc:hover:not(:disabled) {
    transform: translateY(-1px);
    border-color: #4a4e58;
  }
  .sc.act {
    border-color: #7aa7e6;
    background: #16202e;
    color: #e8e8eb;
  }
  .sc:focus-visible,
  .lnk:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .warn {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 10px 12px;
    border-radius: 12px;
    border: 1px solid #5a4a2c;
    background: #1e1a12;
    font-size: 12.5px;
    color: #e6c48f;
  }
  .foot {
    display: flex;
    gap: 14px;
    border-top: 1px solid #25272d;
    padding-top: 6px;
  }
  .lnk {
    font: inherit;
    font-size: 12.5px;
    border: 0;
    background: transparent;
    color: #9a9da6;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 7px;
    height: 32px;
    padding: 0 10px;
    border-radius: 8px;
    text-decoration: none;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .lnk.on {
    color: #e8e8eb;
  }
  .lnk:hover {
    background: #1f2026;
    color: #e8e8eb;
  }
</style>
