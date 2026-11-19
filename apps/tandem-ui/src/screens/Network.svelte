<script lang="ts">
  // Network (`V3-Network`, T-118): the link map, the indicators and one card per path. Everything
  // comes from the agent's network state (`desk.network`); "Measure bandwidth" asks the agent for the
  // 3 s test.
  import { useStore } from "../lib/context";
  import { LINK_NAME, MACHINES, machineName } from "../lib/machines";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Indicators from "./network/Indicators.svelte";
  import LinkMap, { type MapNode } from "./network/LinkMap.svelte";
  import PathCard from "./network/PathCard.svelte";

  const store = useStore();
  const network = $derived(store.desk.network);
  const nodes = $derived(
    MACHINES.map((id): MapNode => {
      const m = store.desk.machines[id];
      return { name: m?.name ?? machineName(id), os: m?.os ?? "windows", color: m?.color ?? (id === "a" ? "#7aa7e6" : "#a99be8") };
    }),
  );

  /** It is being measured: by this button or (another window) according to the agent. */
  let asked = $state(false);
  const measuring = $derived(asked || network.measuring !== null);
  const measuringName = $derived(network.measuring ? LINK_NAME[network.measuring] : null);

  async function measure() {
    asked = true;
    try {
      await store.measureLink();
    } finally {
      asked = false;
    }
  }
</script>

<div class="page">
  <PageHeader title="Network" search={false}>
    {#snippet actions()}
      <Button icon="gauge" motion="tilt" style="--btn-h:38px;--btn-px:15px" disabled={measuring || !store.can("measure")} onclick={measure}>
        {measuring ? `Measuring${measuringName ? ` ${measuringName}` : ""}…` : "Measure bandwidth"}
      </Button>
    {/snippet}
  </PageHeader>
  {#each network.notices as notice (notice.interface)}
    <section class="notice" role="alert">
      <b>{notice.interface}</b>
      <span>{notice.message}</span>
      <code class="mono">{notice.command}</code>
    </section>
  {/each}
  <LinkMap {network} a={nodes[0]!} b={nodes[1]!} />
  <Indicators {network} />
  <section class="paths">
    {#each network.paths as path, i (path.kind)}
      <PathCard {path} delay={0.18 + i * 0.04} />
    {/each}
  </section>
</div>

<style>
  .notice {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 14px;
    border-radius: 10px;
    border: 1px solid #d9a75a;
    background: rgba(217, 167, 90, 0.1);
  }
  .notice code {
    user-select: all;
    word-break: break-all;
  }
  .page {
    display: flex;
    flex-direction: column;
    gap: 14px;
    flex: 1;
  }
  .paths {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
  }
</style>
