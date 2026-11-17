<script lang="ts">
  // Ajustes: base de T-112 (pestañas con íconos de `F-Ajustes`). T-120 las llena y las
  // conecta a la configuración compartida (por ahora solo «General»).
  import { useStore } from "../lib/context";
  import PageHeader from "../shell/PageHeader.svelte";
  import Button from "../ui/Button.svelte";
  import Card from "../ui/Card.svelte";
  import Icon from "../ui/Icon.svelte";
  import type { IconName } from "../ui/icons";

  const store = useStore();
  const TABS: { id: string; label: string; icon: IconName }[] = [
    { id: "general", label: "General", icon: "ajustes" },
  ];
  let tab = $state("general");
</script>

<div class="page">
  <PageHeader title="Ajustes" search={false} />
  <div class="cols">
    <div class="tabs" role="tablist" aria-label="Ajustes" aria-orientation="vertical">
      {#each TABS as t (t.id)}
        <button type="button" role="tab" class="tab" class:on={tab === t.id} aria-selected={tab === t.id} onclick={() => (tab = t.id)}><Icon name={t.icon} />{t.label}</button>
      {/each}
    </div>
    {#key tab}
      <Card class="pane" tag="section" label={TABS.find((t) => t.id === tab)?.label}>
          <div class="row">
            <span class="fill">Cambiar la otra máquina</span>
            <Button icon="refresh" motion="spin" onclick={() => store.openWizard()}>Emparejar de nuevo</Button>
          </div>
          <div class="row">
            <span class="fill">Asistente de configuración</span>
            <Button icon="arrow-right" motion="go" iconAfter onclick={() => store.openWizard()}>Abrir</Button>
          </div>
      </Card>
    {/key}
  </div>
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 16px;
    flex: 1;
  }
  .cols {
    display: grid;
    grid-template-columns: 220px minmax(0, 1fr);
    gap: 18px;
    align-items: start;
  }
  .tabs {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .tab {
    font: inherit;
    text-align: left;
    font-size: 13.5px;
    padding: 9px 12px;
    border-radius: 9px;
    border: 0;
    background: transparent;
    color: #a3a6af;
    cursor: pointer;
    min-height: 38px;
    display: flex;
    align-items: center;
    gap: 10px;
    transition:
      background 0.2s,
      color 0.2s;
  }
  .tab :global(svg) {
    opacity: 0.8;
  }
  .tab:hover {
    background: #17181c;
    color: #e8e8eb;
  }
  .tab.on {
    background: #1c1d22;
    color: #e8e8eb;
  }
  .tab:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .cols :global(.card.pane) {
    animation: fd 0.3s ease-out;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    border-top: 1px solid #1f2126;
    font-size: 13.5px;
  }
  .row:first-child {
    border-top: 0;
  }
  .fill {
    flex: 1;
  }
</style>
