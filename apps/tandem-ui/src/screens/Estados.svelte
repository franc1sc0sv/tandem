<script lang="ts">
  // Estados difíciles, vista de referencia (`V3-Estados`, Ajustes → Avanzado): las nueve
  // tarjetas con los nombres reales del puesto y la acción que arregla cada una.
  import { useStore } from "../lib/context";
  import { stateCards, type StateDo } from "../lib/states";
  import { WIZARD_PAIR } from "../lib/store/store.svelte";
  import StateCard from "../ui/StateCard.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
  const cards = $derived(stateCards(desk));
  const peer = $derived(desk.here === "a" ? "b" : "a");

  function run(does: StateDo) {
    if (does === "add-machine") store.openWizard(WIZARD_PAIR);
    else if (does === "power-on") void store.setPower(peer, true);
    else if (does === "tunnels") store.go({ screen: "tuneles" });
    else if (does === "permission") void store.openPermission("input_monitoring");
    else if (desk.scenes[0]) void store.applyScene(desk.activeScene ?? desk.scenes[0].id);
    else store.toast("Todavía no hay escenas que reintentar", "bad");
  }
</script>

<div class="page">
  <div class="head">
    <h1>Estados difíciles</h1>
    <span class="mut sub">Cómo se ve cada cosa cuando no sale bien. Regla: siempre se dice qué pasó, qué hizo Tandem solo y qué puedes hacer tú.</span>
  </div>
  <div class="grid">
    {#each cards as c, i (c.id)}
      <StateCard spec={c} index={i} onaction={() => c.action && run(c.action.does)} />
    {/each}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 18px;
    flex: 1;
  }
  .head {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  h1 {
    margin: 0;
    font-size: 20px;
    font-weight: 650;
  }
  .sub {
    font-size: 12.5px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
    gap: 14px;
  }
</style>
