<script lang="ts">
  // Acomodo: base de T-112 con los segmentados de lado y alineación (D18). T-117 lo conecta
  // al agente y agrega el dibujo a escala y la prueba de cruce.
  import { useStore } from "../lib/context";
  import PageHeader from "../shell/PageHeader.svelte";
  import Card from "../ui/Card.svelte";
  import Segmented, { type SegOption } from "../ui/Segmented.svelte";

  const store = useStore();
  const peer = $derived(store.desk.machines[store.desk.here === "a" ? "b" : "a"]);

  const SIDES: SegOption[] = [
    { value: "izquierda", label: "Izquierda" },
    { value: "arriba", label: "Arriba" },
    { value: "derecha", label: "Derecha" },
    { value: "abajo", label: "Abajo" },
  ];
  const ALIGN: SegOption[] = [
    { value: "inicio", label: "Inicio" },
    { value: "centro", label: "Centro" },
    { value: "fin", label: "Fin" },
  ];
  let side = $state("izquierda");
  let align = $state("centro");
</script>

<div class="page">
  <PageHeader title="Acomodo" />
  <Card tag="section" label="Dónde está la otra máquina" class="pane">
    <span class="lbl">Lado de {peer?.name ?? "la otra máquina"}</span>
    <Segmented options={SIDES} bind:value={side} label="Lado" />
    <span class="lbl">Alineación</span>
    <Segmented options={ALIGN} bind:value={align} label="Alineación" />
  </Card>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .page :global(.card.pane) {
    display: flex;
    flex-direction: column;
    gap: 10px;
    max-width: 420px;
  }
  .lbl {
    font-size: 12px;
    color: #9a9da6;
  }
</style>
