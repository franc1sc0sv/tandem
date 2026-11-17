<script lang="ts">
  // Página de una máquina: base de T-112 (módulos con su interruptor). T-115 trae recursos,
  // entornos y el resto de `V3-Maquina`.
  import { useStore } from "../lib/context";
  import type { Machine } from "../lib/model";
  import type { ModuleView } from "../lib/store/types";
  import PageHeader from "../shell/PageHeader.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";
  import { machineIcon } from "../ui/icons";
  import Ring from "../ui/Ring.svelte";
  import Switch from "../ui/Switch.svelte";

  interface Props {
    machine: Machine;
  }
  let { machine }: Props = $props();
  const store = useStore();
  const m = $derived(store.desk.machines[machine]);
  const STATE: Record<ModuleView["state"], { text: string; tone: "ok" | "idle" | "warn" }> = {
    active: { text: "activo", tone: "ok" },
    off: { text: "apagado", tone: "idle" },
    unavailable: { text: "no disponible", tone: "warn" },
  };
</script>

<div class="page">
  {#if m}
    <PageHeader title={m.name} />
    <div class="row">
      <Card delay={0} class="id">
        <span class="big"><Icon name={machineIcon(m.os)} size={34} stroke={1.6} color={m.color} /></span>
        <div class="col"><b>{m.name}</b><span class="mut sm">{m.detail}</span></div>
        <span class="fill"></span>
        <Chip tone={m.power === "on" ? "ok" : "idle"} pulse={m.power === "on"}>{m.power === "on" ? "encendida" : "apagada"}</Chip>
      </Card>
      {#if m.resources}
        <Card delay={0.06} class="res">
          <Ring value={m.resources.cpu.percent / 100} color={m.color} label="CPU {m.resources.cpu.label}" />
          <div class="col"><span class="mut sm">CPU</span><b class="mono">{m.resources.cpu.label}</b></div>
          <Ring value={m.resources.ram.percent / 100} color={m.color} label="RAM {m.resources.ram.label}" />
          <div class="col"><span class="mut sm">RAM</span><b class="mono">{m.resources.ram.label}</b></div>
        </Card>
      {/if}
    </div>
    <Card delay={0.12} tag="section" label="Módulos">
      <h2 class="h3">Módulos</h2>
      {#each m.modules as mod (mod.id)}
        <div class="mod">
          <span class="dot {STATE[mod.state].tone}"></span>
          <b>{mod.name}</b>
          <span class="mut det">{mod.detail}</span>
          <Chip tone={STATE[mod.state].tone} dot={false}>{STATE[mod.state].text}</Chip>
          <Switch
            checked={mod.state === "active"}
            label={mod.name}
            disabled={mod.state === "unavailable" || !store.can("modules")}
            onchange={(on) => store.setModule(machine, mod.id, on)}
          />
        </div>
      {:else}
        <p class="mut sm">El agente todavía no informa sus módulos.</p>
      {/each}
    </Card>
  {:else}
    <PageHeader title="Máquina" />
    <Card><span class="mut">Todavía no se conoce esta máquina.</span></Card>
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .sm {
    font-size: 12.5px;
  }
  .fill {
    flex: 1;
  }
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 14px;
  }
  .row :global(.card.id),
  .row :global(.card.res) {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .big {
    display: grid;
  }
  .mod {
    display: grid;
    grid-template-columns: 9px 170px minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 12px;
    padding: 11px 0;
    border-top: 1px solid #1f2126;
    font-size: 13px;
  }
  .mod:first-of-type {
    margin-top: 10px;
  }
  .mod b {
    font-weight: 600;
  }
  .det {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .dot {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: #4a4d55;
  }
  .dot.ok {
    background: #6cc59a;
  }
  .dot.warn {
    background: #d9a75a;
  }
</style>
