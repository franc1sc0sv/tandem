<script lang="ts">
  // Página de una machine (T-115, `V3-Machine`/`F-MachineMac`): nombre y color editables,
  // recursos en vivo, módulos con su interruptor y entornos de ejecución.
  import { tick } from "svelte";
  import { useStore } from "../lib/context";
  import {
    MODULE_TEXT,
    PALETTE,
    checkName,
    cpuSub,
    diskText,
    envLabel,
    gb,
    gpuSub,
    headerSub,
    missingPermission,
    orderEnvs,
    pct,
    ramSub,
    sharedMemory,
    tint,
    trendArea,
    trendPoints,
    vramWarn,
  } from "../lib/machine";
  import type { Machine } from "../lib/model";
  import type { ModuleView } from "../lib/store/types";
  import Button from "../ui/Button.svelte";
  import Icon from "../ui/Icon.svelte";
  import { machineIcon } from "../ui/icons";
  import Switch from "../ui/Switch.svelte";

  interface Props {
    machine: Machine;
  }
  let { machine }: Props = $props();
  const store = useStore();
  const m = $derived(store.desk.machines[machine]);
  const peer = $derived(store.desk.machines[machine === "a" ? "b" : "a"]);
  const live = $derived(m?.live ?? null);
  const hw = $derived(m?.hardware ?? null);
  const warn = $derived(vramWarn(live));
  const envs = $derived(orderEnvs(m?.environments ?? []));
  const tunnels = $derived(m?.modules.find((x) => x.id === "tunnels") ?? null);
  // La fila «Permisos» (F-MachineMac): solo en la página de this machine, si falta alguno.
  const perm = $derived(m?.here ? missingPermission(store.desk.permissions.rows.filter((r) => r.state === "missing").map((r) => r.id)) : null);

  const WOL_MISSING = "Its network card’s MAC is missing: it is learned when pairing; if it did not arrive, type it here (aa:bb:cc:dd:ee:ff).";
  const wolMac = $derived(m?.wake?.mac ?? null);
  let macDraft = $state("");
  $effect(() => {
    macDraft = wolMac ?? "";
  });

  const fmt = (n: number | null | undefined, unit: string) => (n === null || n === undefined ? "—" : `${Math.round(n)} ${unit}`);

  // ---- Nombre ----
  let editing = $state(false);
  let draft = $state("");
  let input: HTMLInputElement | undefined = $state();

  async function startEdit() {
    if (!m || !store.can("machine")) return;
    draft = m.name;
    editing = true;
    await tick();
    input?.select();
  }

  async function saveName() {
    if (!editing || !m) return;
    editing = false;
    const c = checkName(draft);
    if (!c.ok) {
      store.toast(c.reason, "bad");
      return;
    }
    if (c.name !== m.name) await store.editMachine(machine, { name: c.name });
  }

  function nameKey(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      void saveName();
    } else if (e.key === "Escape") {
      e.preventDefault();
      editing = false;
    }
  }

  // ---- Color ----
  const taken = (color: string) => !!peer && peer.color.toLowerCase() === color.toLowerCase();

  function pickColor(color: string) {
    if (!m || taken(color) || m.color.toLowerCase() === color.toLowerCase()) return;
    void store.editMachine(machine, { color });
  }

  // Radiogroup: las flechas mueven la elección (saltando el color de the other machine).
  function colorKey(e: KeyboardEvent) {
    if (!m || !["ArrowRight", "ArrowDown", "ArrowLeft", "ArrowUp"].includes(e.key)) return;
    e.preventDefault();
    const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : -1;
    const i = PALETTE.findIndex((p) => p.color.toLowerCase() === m?.color.toLowerCase());
    for (let k = 1; k <= PALETTE.length; k++) {
      const next = PALETTE[(i + step * k + PALETTE.length * 2) % PALETTE.length];
      if (next && !taken(next.color)) {
        pickColor(next.color);
        const group = (e.currentTarget as HTMLElement).querySelectorAll<HTMLButtonElement>("[role=radio]");
        group[PALETTE.indexOf(next)]?.focus();
        return;
      }
    }
  }

  // ---- Módulos ----
  let busy: string | null = $state(null);
  const canSwitch = (mod: ModuleView) => mod.switchable && mod.state !== "unavailable" && store.can("modules");

  async function toggle(mod: ModuleView) {
    if (!canSwitch(mod) || busy) return;
    busy = mod.id;
    try {
      await store.setModule(machine, mod.id, mod.state !== "active");
    } finally {
      busy = null;
    }
  }

  // ---- Entornos ----
  let scanning = $state(false);
  async function rescan() {
    if (scanning) return;
    scanning = true;
    try {
      await store.rescan(machine);
    } finally {
      scanning = false;
    }
  }
</script>

<div class="page">
  {#if m}
    <header class="hd">
      <Icon name={machineIcon(m.os)} size={34} stroke={1.6} color={m.color} />
      <div class="who">
        <h1>
          {#if editing}
            <input
              bind:this={input}
              bind:value={draft}
              class="name-in"
              aria-label="Machine name"
              maxlength={40}
              onkeydown={nameKey}
              onblur={() => void saveName()}
            />
          {:else}
            <button type="button" class="name" title="Change the name" disabled={!store.can("machine")} onclick={startEdit}
              >{m.name}<Icon name="edit" size={14} stroke={2} class="pen" /></button
            >
          {/if}
        </h1>
        <span class="mut sub">{headerSub(m)}</span>
      </div>
      <span class="fill"></span>
      <div class="colors" role="radiogroup" aria-label="Machine color" tabindex="-1" onkeydown={colorKey}>
        {#each PALETTE as p (p.color)}
          {@const on = p.color.toLowerCase() === m.color.toLowerCase()}
          <button
            type="button"
            role="radio"
            class="dot-c"
            class:on
            aria-checked={on}
            aria-label={p.name}
            title={taken(p.color) ? `It is ${peer?.name}’s color` : p.name}
            tabindex={on ? 0 : -1}
            disabled={taken(p.color) || !store.can("machine")}
            style="background:{p.color}"
            onclick={() => pickColor(p.color)}
          ></button>
        {/each}
      </div>
      {#if !m.here && store.can("power")}
        {#if m.power === "on"}
          <Button icon="power" variant="danger" style="--btn-h:38px;--btn-px:15px" onclick={() => store.setPower(machine, false)}>Power off</Button>
        {:else if m.power === "off"}
          <span title={wolMac ? undefined : WOL_MISSING}><Button icon="power" style="--btn-h:38px;--btn-px:15px" disabled={!wolMac} onclick={() => store.setPower(machine, true)}>Power on</Button></span>
        {/if}
      {/if}
    </header>

    {#if m.wake}
      <section class="wol" aria-label="Wake-on-LAN">
        <label for="wol-mac"><b>Wake-on-LAN</b></label>
        {#if !wolMac}<span class="mut s12">{WOL_MISSING}</span>{/if}
        <input id="wol-mac" class="mono" placeholder="aa:bb:cc:dd:ee:ff" bind:value={macDraft} aria-label="MAC of {m.name}" />
        <Button icon="check" size="sm" disabled={macDraft.trim() === (wolMac ?? "")} onclick={() => void store.setWol(macDraft.trim() || null)}>Save</Button>
        {#if wolMac}<Button icon="trash" variant="ghost" size="sm" iconOnly label="Clear MAC" onclick={() => void store.setWol(null)} />{/if}
      </section>
    {/if}

    {#if live || hw}
      {@const gpu = live?.gpu ?? null}
      <section class="res" class:two={!hw?.gpu && !gpu} aria-label="Live resources">
        <div class="grp">
          <div class="gh">
            <span class="gi" style="background:{tint(m.color)}"><Icon name="cpu" size={16} stroke={1.8} color={m.color} /></span><b>CPU</b><span class="mut s115"
              >{cpuSub(hw, m.os)}</span
            >
          </div>
          <div class="val"><b class="mono big">{fmt(live?.cpu.percent, "%")}</b>{#if live?.cpu.temp != null}<span class="mut mono s12">{live.cpu.temp} °C</span>{/if}</div>
          {#if live}
            <svg viewBox="0 0 120 34" width="100%" height="34" preserveAspectRatio="none" aria-hidden="true"
              ><path d={trendArea(live.cpu.history, 120, 34)} fill={m.color} fill-opacity=".14"></path><polyline
                class="ln"
                points={trendPoints(live.cpu.history, 120, 34)}
                fill="none"
                stroke={m.color}
                stroke-width="1.6"
              ></polyline></svg
            >
          {/if}
        </div>
        <div class="grp" style="animation-delay:.06s">
          <div class="gh">
            <span class="gi" style="background:{tint(m.color)}"><Icon name="ram" size={16} stroke={1.8} color={m.color} /></span><b>RAM</b><span class="mut s115"
              >{ramSub(hw, live)}</span
            >
          </div>
          {#if live}
            <div class="val"><b class="mono big">{gb(live.ram.used)}</b><span class="mut mono s12">/ {gb(live.ram.total)} GB</span></div>
            <div class="bar2"><span style="width:{pct(live.ram.used, live.ram.total)}%;background:{m.color}"></span></div>
            <svg viewBox="0 0 120 22" width="100%" height="22" preserveAspectRatio="none" aria-hidden="true"
              ><polyline class="ln" points={trendPoints(live.ram.history, 120, 22)} fill="none" stroke={m.color} stroke-width="1.5"></polyline></svg
            >
          {:else}
            <div class="val"><b class="mono big">—</b></div>
          {/if}
        </div>
        {#if hw?.gpu || gpu}
          <div class="grp" class:warn style="animation-delay:.12s">
            <div class="gh">
              <span class="gi" style="background:{tint(m.color)}"><Icon name="gpu" size={16} stroke={1.8} color={m.color} /></span><b>GPU</b><span class="mut s115"
                >{gpuSub(hw)}</span
              >
              {#if warn}<span class="fill"></span><span class="warnchip"><Icon name="alert" size={12} stroke={2.2} />VRAM almost full</span>{/if}
            </div>
            <div class="g3">
              <div class="gc">
                <span class="mut s115">Usage</span><b class="mono big">{fmt(gpu?.percent, "%")}</b>
                {#if gpu && gpu.history.length > 0}
                  <svg viewBox="0 0 120 26" width="100%" height="26" preserveAspectRatio="none" aria-hidden="true"
                    ><polyline class="ln" points={trendPoints(gpu.history, 120, 26)} fill="none" stroke={m.color} stroke-width="1.6"></polyline></svg
                  >
                {/if}
              </div>
              {#if sharedMemory(hw)}
                <div class="gc">
                  <span class="mut s115">Memory</span><b class="mono big"
                    >{gpu?.vramUsed != null ? gb(gpu.vramUsed) : "—"}<span class="mut unit">{" GB"}</span></b
                  ><span class="mut s115">of RAM</span>
                </div>
              {:else}
                <div class="gc">
                  <span class="s115" class:mut={!warn} class:amber={warn}>VRAM</span>
                  {#if gpu?.vramUsed != null && gpu.vramTotal}
                    <b class="mono big">{gb(gpu.vramUsed)}<span class="mut unit">{` / ${gb(gpu.vramTotal)} GB`}</span></b>
                    <div class="bar2"><span style="width:{pct(gpu.vramUsed, gpu.vramTotal)}%;background:{warn ? '#d9a75a' : m.color}"></span></div>
                  {:else}
                    <b class="mono big">—</b>
                  {/if}
                </div>
              {/if}
              <div class="gc">
                <span class="mut s115">Temperature</span><b class="mono big">{fmt(gpu?.temp, "°C")}</b>
                {#if gpu?.fans != null}<span class="mut s115">fans {gpu.fans} %</span>{:else if hw?.fanless}<span class="mut s115">fanless</span>{/if}
              </div>
            </div>
          </div>
        {/if}
      </section>
    {/if}
    {#if live?.disk || live?.battery}
      <div class="disk mut">
        {#if live.disk}
          <Icon name="disk" size={15} stroke={1.8} />Disk<div class="bar2 dbar" class:short={!!live.battery}>
            <span style="width:{100 - pct(live.disk.free, live.disk.total)}%;background:#6a6d76"></span>
          </div>
          <span class="mono">{diskText(live.disk.free, live.disk.total)}</span>
        {/if}
        {#if live.battery}
          {#if live.disk}<span class="gap"></span>{/if}
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"
            ><rect x="2" y="7" width="18" height="10" rx="2"></rect><path d="M22 11v2"></path><rect
              x="4"
              y="9"
              width={Math.max(1, Math.round((12 * live.battery.percent) / 100))}
              height="6"
              rx="1"
              fill={live.battery.percent <= 20 ? "#d9a75a" : "#6cc59a"}
              stroke="none"
            ></rect></svg
          >Battery<span class="mono">{Math.round(live.battery.percent)} % · {live.battery.plugged ? "plugged in" : "on battery"}</span>
        {/if}
      </div>
    {/if}

    <!-- La página de this machine usa la lista compacta de `F-MachineMac`; la de la otra, la de
         `V3-Machine`. -->
    <div class="lower" class:compact={m.here}>
      <section class="card mods" aria-label="Modules">
        <div class="mh"><h2 class="h3">Modules</h2></div>
        {#each m.modules as mod, i (mod.id)}
          <div class="mod">
            <span class="d {mod.state}"></span>
            <b>{mod.name}</b>
            <span class:mut={mod.state !== "unavailable"} class:amber={mod.state === "unavailable"}>{mod.detail}</span>
            {#if canSwitch(mod)}
              <button
                type="button"
                class="chip tog {mod.state}"
                role="switch"
                aria-checked={mod.state === "active"}
                aria-label={mod.name}
                aria-busy={busy === mod.id}
                title={mod.state === "active" ? `Turn off ${mod.name}` : `Turn on ${mod.name}`}
                onclick={() => toggle(mod)}>{MODULE_TEXT[mod.state]}</button
              >
            {:else}
              <span class="chip {mod.state}" title={mod.state === "unavailable" ? mod.detail : undefined}>{MODULE_TEXT[mod.state]}</span>
            {/if}
          </div>
          {#if i === 0 && perm}
            {@const id = perm.id}
            <div class="mod">
              <span class="d unavailable"></span>
              <b>Permissions</b>
              <span class="amber">{perm.text}</span>
              <Button icon="external" variant="pri" size="sm" motion="lift" style="--btn-h:34px;--btn-px:13px" onclick={() => store.openPermission(id)}>Open settings</Button>
            </div>
          {/if}
        {/each}
      </section>

      <section class="card envs" aria-label="Environments">
        <div class="eh">
          <h2 class="h3">Execution environments</h2>
          <span class="fill"></span>
          <button
            type="button"
            class="rescan"
            class:spin={scanning}
            aria-label="Scan again"
            title="Scan again"
            aria-busy={scanning}
            disabled={m.power !== "on"}
            onclick={rescan}><Icon name="refresh" size={15} stroke={2} /></button
          >
        </div>
        {#if envs.main}
          <div class="env main" style="border-color:{m.color};background:{tint(m.color)}">
            <span class="en"
              ><b>{envLabel(envs.main)}</b><span class="fill"></span>{#if m.here}<span class="mut s11">default</span>{:else}<span class="chip s11"
                  >default</span
                >{/if}</span
            >
            <span class="mono mut s115">{envs.main.detail}</span>
          </div>
        {/if}
        {#each envs.rest as e (e.id)}
          <div class="env"><b>{envLabel(e)}</b><span class="mono mut s115">{e.detail}</span></div>
        {/each}
        {#if tunnels && !m.here}
          <div class="remote">
            <span>Allow remote execution</span><span class="fill"></span>
            <Switch
              checked={tunnels.state === "active"}
              label="Allow remote execution"
              disabled={!canSwitch(tunnels) || busy !== null}
              onchange={() => toggle(tunnels)}
            />
          </div>
        {/if}
      </section>
    </div>
  {:else}
    <header class="hd"><h1>Machine</h1></header>
    <section class="card"><span class="mut">This machine is not known yet.</span></section>
  {/if}
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
  .mut {
    color: #9a9da6;
  }
  .amber {
    color: #e6c48f;
  }
  .mono {
    font-family: "JetBrains Mono", ui-monospace, monospace;
  }
  .s115 {
    font-size: 11.5px;
  }
  .s12 {
    font-size: 12px;
  }
  .s11,
  .chip.s11 {
    font-size: 11px;
  }
  .card {
    background: #15161a;
    border: 1px solid #25272d;
    border-radius: 14px;
    padding: 16px;
    min-width: 0;
  }
  .h3 {
    margin: 0;
    font-size: 13px;
    font-weight: 600;
    color: #e8e8eb;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 4px 9px;
    border-radius: 999px;
    border: 1px solid #31343b;
    font-size: 12px;
    color: #c4c6cc;
    white-space: nowrap;
  }
  .chip.off {
    color: #9a9da6;
  }
  .chip.unavailable {
    color: #e6c48f;
    border-color: #5a4a2c;
  }

  /* Cabecera: ícono, nombre editable, color y apagar. */
  .hd {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .name {
    font: inherit;
    color: inherit;
    background: none;
    border: 0;
    padding: 0;
    cursor: text;
    position: relative;
    border-radius: 4px;
  }
  .name :global(.pen) {
    position: absolute;
    left: 100%;
    top: 50%;
    margin: -7px 0 0 8px;
    color: #9a9da6;
    opacity: 0;
    transition: opacity 0.2s;
  }
  .name:hover:not(:disabled) :global(.pen),
  .name:focus-visible :global(.pen) {
    opacity: 1;
  }
  .name:disabled {
    cursor: default;
  }
  .name:focus-visible,
  .rescan:focus-visible,
  .tog:focus-visible,
  .dot-c:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .name-in {
    font: inherit;
    color: #e8e8eb;
    background: #111215;
    border: 1px solid #31343b;
    border-radius: 8px;
    padding: 0 6px;
    margin: -1px -7px;
    width: 16em;
  }
  .sub {
    font-size: 12.5px;
  }
  .colors {
    display: flex;
    gap: 6px;
  }
  .dot-c {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    border: 2px solid #0e0f12;
    cursor: pointer;
    padding: 0;
    transition:
      border-color 0.2s,
      transform 0.2s;
  }
  .dot-c:hover:not(:disabled) {
    transform: scale(1.1);
  }
  .dot-c.on {
    border-color: #e8e8eb;
  }
  .dot-c:disabled {
    cursor: not-allowed;
  }

  /* Recursos agrupados (V3-Machine). */
  .wol {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 10px 2px;
    flex-wrap: wrap;
  }
  .wol input {
    min-width: 190px;
    padding: 5px 8px;
    border-radius: 8px;
    border: 1px solid var(--line, #2a2d33);
    background: transparent;
    color: inherit;
  }
  .res {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1.6fr);
    gap: 12px;
  }
  .res.two {
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  }
  .grp {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 16px;
    border-radius: 14px;
    background: #15161a;
    border: 1px solid #25272d;
    min-width: 0;
    animation: up 0.45s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    transition:
      border-color 0.2s,
      transform 0.2s;
  }
  .grp:hover {
    transform: translateY(-2px);
    border-color: #3a3d45;
  }
  .grp.warn {
    border-color: #3d3524;
  }
  .gh {
    display: flex;
    align-items: center;
    gap: 9px;
    font-size: 13px;
  }
  .gh b {
    font-weight: 600;
  }
  .gi {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    flex: none;
  }
  .val {
    display: flex;
    align-items: baseline;
    gap: 8px;
  }
  .big {
    font-size: 24px;
    font-weight: 500;
    letter-spacing: -0.01em;
  }
  .unit {
    font-size: 12px;
    font-weight: 400;
  }
  .g3 {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 14px;
  }
  .gc {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .bar2 {
    height: 6px;
    border-radius: 3px;
    background: #24262c;
    overflow: hidden;
  }
  .bar2 span {
    display: block;
    height: 100%;
    border-radius: 3px;
    animation: grow 1.1s cubic-bezier(0.2, 0.8, 0.2, 1) both;
    transition: width 0.6s cubic-bezier(0.2, 0.8, 0.2, 1);
  }
  .ln {
    stroke-dasharray: 300;
    stroke-dashoffset: 300;
    animation: dr 1.6s ease-out 0.2s forwards;
  }
  .warnchip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    height: 24px;
    padding: 0 9px;
    border-radius: 999px;
    font-size: 11.5px;
    color: #e6c48f;
    border: 1px solid #5a4a2c;
    background: #1e1a12;
    animation: wp 2.4s ease-in-out infinite;
  }
  .disk {
    display: flex;
    align-items: center;
    gap: 12px;
    font-size: 12.5px;
    padding: 0 4px;
    flex-wrap: wrap;
  }
  .dbar {
    flex: 0 1 220px;
  }
  .dbar.short {
    flex-basis: 200px;
  }
  .gap {
    width: 16px;
  }

  /* Módulos y entornos, estirados al mismo alto. */
  .lower {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    align-items: stretch;
  }
  .mods {
    flex: 999 1 520px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  /* Compacta (F-MachineMac): filas de 37 px; el chip no las agranda. */
  .compact .mods {
    gap: 0;
  }
  .compact .mod .chip {
    margin: -5px 0;
  }
  .mh {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-bottom: 8px;
  }
  .mod {
    display: grid;
    grid-template-columns: 22px minmax(120px, 170px) minmax(0, 1fr) auto;
    gap: 12px;
    align-items: center;
    padding: 10px 4px;
    border-top: 1px solid #1f2126;
    font-size: 13px;
    animation: up 0.4s ease-out both;
  }
  .mod b {
    font-weight: 600;
  }
  .d {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    box-sizing: border-box;
  }
  .d.active {
    background: #6cc59a;
  }
  .d.off {
    border: 1.5px dashed #6a6d76;
  }
  .d.unavailable {
    background: #d9a75a;
    animation: pl 1.6s infinite;
  }
  .tog {
    font: inherit;
    font-size: 12px;
    background: none;
    cursor: pointer;
    transition:
      border-color 0.2s,
      color 0.2s;
  }
  .tog:hover {
    border-color: #4a4e58;
    color: #e8e8eb;
  }
  .tog[aria-busy="true"] {
    opacity: 0.6;
  }
  .envs {
    flex: 1 1 300px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .eh {
    display: flex;
    align-items: center;
  }
  /* El botón de re-escanear no agranda la fila del título. */
  .rescan {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    margin: -4px -4px -4px 0;
    padding: 0;
    border: 0;
    border-radius: 6px;
    background: none;
    color: #9a9da6;
    cursor: pointer;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .rescan:hover:not(:disabled) {
    background: #1f2026;
    color: #e8e8eb;
  }
  .rescan:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .rescan.spin :global(svg) {
    animation: spin 0.9s linear infinite;
  }
  .env {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid #25272d;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .env b {
    font-weight: 600;
  }
  .en {
    display: flex;
    gap: 8px;
    align-items: center;
  }
  .remote {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-top: 12px;
    margin-top: auto;
    border-top: 1px solid #1f2126;
    font-size: 13px;
  }

  @keyframes up {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes dr {
    to {
      stroke-dashoffset: 0;
    }
  }
  @keyframes grow {
    from {
      width: 0;
    }
  }
  @keyframes wp {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(217, 167, 90, 0.35);
    }
    50% {
      box-shadow: 0 0 0 4px rgba(217, 167, 90, 0);
    }
  }
  @keyframes pl {
    0%,
    100% {
      box-shadow: 0 0 0 0 rgba(217, 167, 90, 0.5);
    }
    50% {
      box-shadow: 0 0 0 5px rgba(217, 167, 90, 0);
    }
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .ln {
      stroke-dashoffset: 0;
    }
  }
</style>
