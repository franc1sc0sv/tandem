<script lang="ts">
  // Red: base de T-112 (camino actual y medir el enlace). T-118 trae caminos, eventos y
  // prioridades de `V3-Red`.
  import { useStore } from "../lib/context";
  import { LINK_NAME } from "../lib/machines";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Ring from "../ui/Ring.svelte";

  const store = useStore();
  const link = $derived(store.desk.link);
  let measuring = $state(false);

  async function measure() {
    measuring = true;
    await store.measureLink();
    measuring = false;
  }
</script>

<div class="page">
  <PageHeader title="Red">
    {#snippet actions()}
      <Button icon="refresh" motion="spin" disabled={measuring || !store.can("measure")} onclick={measure}>Medir ancho de banda</Button>
    {/snippet}
  </PageHeader>
  <Card tag="section" label="Camino actual" class="now">
    <Ring value={link.gbps !== null ? Math.min(1, link.gbps / 10) : 0} color="#6cc59a" label="Ancho de banda" />
    <div class="col">
      <span class="mut sm">Camino de la entrada</span>
      <b>{link.kind ? LINK_NAME[link.kind] : "Sin enlace"}</b>
    </div>
    <span class="fill"></span>
    <span class="mono mut">{link.gbps !== null ? `${link.gbps} Gbps` : "—"} · {link.ms !== null ? `${link.ms} ms` : "—"}</span>
    <Chip tone={link.kind ? "ok" : "bad"} pulse={link.kind !== null}>{link.kind ? "en uso" : "caído"}</Chip>
  </Card>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
  }
  .page :global(.card.now) {
    display: flex;
    align-items: center;
    gap: 14px;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .sm {
    font-size: 12px;
  }
  .fill {
    flex: 1;
  }
</style>
