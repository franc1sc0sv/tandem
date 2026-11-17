<script lang="ts">
  // Monitores y escenas: base de T-112 (escenas aplicables). T-116 trae los monitores, la
  // entrada de cada uno y el editor de escenas.
  import { useStore } from "../lib/context";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Chip from "../ui/Chip.svelte";
  import Kbd from "../ui/Kbd.svelte";

  const store = useStore();
  const desk = $derived(store.desk);
</script>

<div class="page">
  <PageHeader title="Monitores" search={false} />
  <div class="grid">
    {#each desk.scenes as s, i (s.id)}
      {@const on = desk.activeScene === s.id}
      {@const shows = desk.machines[s.shows]}
      <Card delay={i * 0.06} class="sc {on ? 'act' : ''}" tag="article" label="Escena {s.name}">
        <div class="top"><b>{s.name}</b><span class="fill"></span><Kbd raised>{s.key}</Kbd></div>
        <span class="mut sm">{s.desc}</span>
        <div class="bot">
          {#if shows}<Chip dot={false}><span class="sw" style="background:{shows.color}"></span>{shows.name}</Chip>{/if}
          <span class="fill"></span>
          {#if on}
            <Chip tone="info" pulse>Activa</Chip>
          {:else}
            <Button icon="play" size="sm" variant="pri" disabled={!store.can("scenes")} onclick={() => store.applyScene(s.id)}>Aplicar</Button>
          {/if}
        </div>
      </Card>
    {/each}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 14px;
  }
  .grid :global(.card.sc) {
    display: flex;
    flex-direction: column;
    gap: 12px;
    background: #111215;
    transition:
      border-color 0.25s,
      transform 0.25s,
      box-shadow 0.25s;
  }
  .grid :global(.card.sc:hover) {
    transform: translateY(-3px);
    border-color: #3a3d45;
    box-shadow: 0 12px 26px rgba(0, 0, 0, 0.35);
  }
  .grid :global(.card.sc.act) {
    border-color: #4f7cbc;
    background: linear-gradient(180deg, #141c28, #111215);
    box-shadow:
      0 0 0 1px rgba(122, 167, 230, 0.2),
      0 0 26px rgba(122, 167, 230, 0.12);
  }
  .top,
  .bot {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .fill {
    flex: 1;
  }
  .sm {
    font-size: 12.5px;
  }
  .sw {
    width: 7px;
    height: 7px;
    border-radius: 50%;
  }
</style>
