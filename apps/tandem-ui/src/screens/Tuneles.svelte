<script lang="ts">
  // Túneles (solo lectura, D9): base de T-112. T-114 trae procesos y salida en vivo.
  import { shortName } from "../lib/commands";
  import { useStore } from "../lib/context";
  import type { TunnelView } from "../lib/store/types";
  import PageHeader from "../shell/PageHeader.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Icon from "../ui/Icon.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const projects = $derived(
    [...new Set(desk.tunnels.map((t) => t.project))].map((p) => ({ name: p, tunnels: desk.tunnels.filter((t) => t.project === p) })),
  );
  const name = (id: "a" | "b") => shortName(desk.machines[id]?.name ?? id.toUpperCase());
  const STATE: Record<TunnelView["state"], { text: string; tone: "info" | "idle" }> = {
    busy: { text: "activo", tone: "info" },
    idle: { text: "inactivo", tone: "idle" },
    closed: { text: "cerrado", tone: "idle" },
  };
</script>

<div class="page">
  <PageHeader title="Túneles" />
  {#each projects as p, i (p.name)}
    <Card delay={i * 0.1} tag="section" label="Proyecto {p.name}">
      <div class="ph"><Icon name="folder" color="#9a9da6" /><b>{p.name}</b><span class="mut sm">{p.tunnels.filter((t) => t.state !== "closed").length} abiertos de {p.tunnels.length}</span></div>
      {#each p.tunnels as t (t.id)}
        <div class="tun" class:closed={t.state === "closed"}>
          <Icon name="sparkle" size={14} color={t.state === "closed" ? "#4a4d55" : "#cbbff5"} />
          <span class="tt">{t.title}</span>
          <span class="mono mut sm">{name(t.from)} → {name(t.to)}</span>
          <span class="mut sm">{t.processes} {t.processes === 1 ? "proceso" : "procesos"}</span>
          <Chip tone={STATE[t.state].tone} pulse={t.state === "busy"}>{STATE[t.state].text}</Chip>
        </div>
      {/each}
    </Card>
  {:else}
    <Card><span class="mut">Sin túneles abiertos</span></Card>
  {/each}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .sm {
    font-size: 12px;
  }
  .ph {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-bottom: 8px;
  }
  .tun {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) 190px 100px 96px;
    align-items: center;
    gap: 12px;
    padding: 10px 0;
    border-top: 1px solid #1f2126;
    font-size: 13px;
  }
  .tun.closed {
    opacity: 0.6;
  }
  .tt {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
</style>
