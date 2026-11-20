<script lang="ts">
  // Settings → Updates (`F-Settings`, T-122): the version of the two machines, search
  // now, "Update all" (the other one first, then this one) and "Update automatically".
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import { availableRow, checkLabel, upToDate, versionRow, type UpdateView } from "../lib/updates";
  import Button from "../ui/Button.svelte";
  import Switch from "../ui/Switch.svelte";

  const store = useStore();
  // The Settings buttons measure 34 px (`F-Settings`).
  const B34 = "--btn-h:34px;--btn-px:13px;--btn-gap:7px";

  let view = $state<UpdateView | null>(null);
  let checkedOnce = $state(false);
  let error = $state<string | null>(null);
  /** "Update all" was pressed and its reply has not come yet: it is not pressed twice. */
  let starting = $state(false);
  const updating = $derived(starting || (view !== null && view.step !== null));

  const version = $derived(view ? versionRow(view) : null);
  const found = $derived(view ? availableRow(view) : null);
  const shownError = $derived(error ?? view?.error ?? null);

  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

  onMount(() => {
    const listen = (v: UpdateView) => (view = v);
    store.updates(listen).then(
      (v) => (view = v),
      (e) => (error = message(e)),
    );
    return () => store.offUpdates(listen);
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
    if (updating) return;
    error = null;
    starting = true;
    try {
      view = await store.updateAll();
    } catch (e) {
      error = message(e);
    } finally {
      starting = false;
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
    <Button icon="refresh" size="sm" style={B34} motion="spin" disabled={view.checking || updating} onclick={check}>{checkLabel(view, checkedOnce)}</Button>
  </div>
  {#if found}
    <div class="row appear">
      <span class="fill col">{found.title}<span class="mut sub">{found.detail}</span></span>
      <Button icon="download" variant="pri" size="sm" style={B34} disabled={updating} onclick={updateAll}>Update all</Button>
    </div>
  {:else if upToDate(view, checkedOnce)}
    <div class="row appear"><span class="fill mut">There are no new versions.</span></div>
  {/if}
  <div class="row">
    <span class="fill">Update automatically</span>
    <Switch checked={view.auto} label="Update automatically" onchange={setAuto} />
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
