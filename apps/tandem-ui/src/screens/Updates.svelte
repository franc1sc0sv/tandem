<script lang="ts">
  // Ajustes → Actualizaciones (`F-Settings`, T-122): la versión de las dos máquinas, buscar
  // ahora, «Update all» (primero la otra, después esta) y «Update only».
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import { availableRow, checkLabel, upToDate, versionRow, type UpdateView } from "../lib/updates";
  import Button from "../ui/Button.svelte";
  import Switch from "../ui/Switch.svelte";

  const store = useStore();
  // Los botones de Ajustes miden 34 px (`F-Settings`).
  const B34 = "--btn-h:34px;--btn-px:13px;--btn-gap:7px";

  let view = $state<UpdateView | null>(null);
  let checkedOnce = $state(false);
  let error = $state<string | null>(null);

  const version = $derived(view ? versionRow(view) : null);
  const found = $derived(view ? availableRow(view) : null);
  const shownError = $derived(error ?? view?.error ?? null);

  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

  onMount(async () => {
    try {
      view = await store.updates((v) => (view = v));
    } catch (e) {
      error = message(e);
    }
  });

  async function check() {
    error = null;
    if (view) view = { ...view, checking: true };
    try {
      view = await store.checkUpdates();
      checkedOnce = true;
    } catch (e) {
      error = message(e);
      if (view) view = { ...view, checking: false };
    }
  }

  async function updateAll() {
    error = null;
    try {
      view = await store.updateAll();
    } catch (e) {
      error = message(e);
    }
  }

  async function setAuto(on: boolean) {
    error = null;
    try {
      view = await store.setAutoUpdate(on);
    } catch (e) {
      error = message(e);
      if (view) view = { ...view, auto: !on };
    }
  }
</script>

{#if view && version}
  <div class="row first">
    <span class="fill col">{version.title}<span class="mut sub">{version.detail}</span></span>
    <Button icon="refresh" size="sm" style={B34} motion="spin" disabled={view.checking || view.step !== null} onclick={check}>{checkLabel(view, checkedOnce)}</Button>
  </div>
  {#if found}
    <div class="row appear">
      <span class="fill col">{found.title}<span class="mut sub">{found.detail}</span></span>
      <Button icon="download" variant="pri" size="sm" style={B34} disabled={view.step !== null} onclick={updateAll}>Update all</Button>
    </div>
  {:else if upToDate(view, checkedOnce)}
    <div class="row appear"><span class="fill mut">There are no new versions.</span></div>
  {/if}
  <div class="row">
    <span class="fill">Update only</span>
    <Switch checked={view.auto} label="Update only" onchange={setAuto} />
  </div>
  <div class="row">
    <span class="fill">Source</span><span class="mono mut src">GitHub Releases · unsigned</span>
  </div>
{/if}
{#if shownError}
  <p class="err" role="alert">{shownError}</p>
{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    border-top: 1px solid #1f2126;
    font-size: 13.5px;
  }
  .row.first {
    border-top: 0;
  }
  .fill {
    flex: 1;
  }
  .col {
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .sub,
  .src {
    font-size: 12.5px;
  }
  .appear {
    animation: fd 0.3s;
  }
  .err {
    margin: 4px 0 0;
    font-size: 12.5px;
    color: var(--amber);
  }
</style>
