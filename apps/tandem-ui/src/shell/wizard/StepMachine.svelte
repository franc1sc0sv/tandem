<script lang="ts">
  // Step 1 (`F-Setup1`): name and color of this machine and what Tandem found on it.
  import { useStore } from "../../lib/context";
  import { PALETTE } from "../../lib/machine";
  import { foundRows } from "../../lib/wizard";
  import Button from "../../ui/Button.svelte";

  const store = useStore();
  const here = $derived(store.desk.machines[store.desk.here]);
  const rows = $derived(foundRows(here ?? null, store.admin));
  // The real state of the service is given by the app, not the module: it is read when the step opens.
  $effect(() => {
    void store.refreshAdmin();
  });
  let draft = $state<string | null>(null);
  const name = $derived(draft ?? here?.name ?? "");

  /** The name is saved on leaving the field or with Enter; empty goes back to the previous one. */
  async function commit() {
    const text = (draft ?? "").trim();
    draft = null;
    if (!here || !text || text === here.name) return;
    await store.editMachine(here.id, { name: text });
  }
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Step 1 of 6</span>
  <h1>This machine</h1>
  <p class="wz-mut wz-lead">Tandem checks what this machine can do. Nothing is fixed: what it finds is what it announces to the rest of the desk.</p>
</div>
<div style="display:flex;gap:18px;flex-wrap:wrap;align-items:flex-end">
  <label style="display:flex;flex-direction:column;gap:6px;font-size:13px;flex:1 1 260px"
    >Name
    <input
      class="wz-input"
      value={name}
      maxlength="40"
      oninput={(e) => (draft = e.currentTarget.value)}
      onblur={commit}
      onkeydown={(e) => e.key === "Enter" && e.currentTarget.blur()}
    />
  </label>
  <div role="radiogroup" aria-label="Color" style="display:flex;gap:8px;padding-bottom:6px">
    {#each PALETTE as c (c.color)}
      {@const on = here?.color === c.color}
      <button
        type="button"
        role="radio"
        class="wz-sw"
        aria-checked={on}
        aria-label={c.name}
        onclick={() => here && store.editMachine(here.id, { color: c.color })}
        style="background:{c.color};border:2px solid {on ? '#e8e8eb' : '#0e0f12'}"
      ></button>
    {/each}
  </div>
</div>
<section aria-label="What it found" class="wz-list">
  <div class="wz-lt"><i style="background:{here?.color ?? '#7aa7e6'}"></i>What it found</div>
  {#if !here}
    <div class="wz-chk"><span class="wz-off"></span>Not connected to tandemd<span class="wz-fill"></span><span class="wz-mut wz-xs">{store.desk.troubleText}</span></div>
  {/if}
  {#each rows as r, i (r.id)}
    <div class="wz-chk" class:service={r.id === "service"} style="animation-delay:{(0.1 + 0.4 * i).toFixed(1)}s">
      {#if r.state === "ok"}
        <span class="wz-ok"
          ><svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="#6cc59a" stroke-width="3" aria-hidden="true"><path d="m4 12 5 5L20 6"></path></svg></span
        >
      {:else if r.state === "off"}
        <span class="wz-off"></span>
      {:else}
        <span class="wz-spin"></span>
      {/if}
      {r.label}<span class="wz-fill"></span>{#if r.note}<span class="wz-mut wz-xs" class:wz-mono={r.mono}>{r.note}</span>{/if}{#if r.install}<Button icon="download" variant="pri" size="sm" class="wz-inst" disabled={store.adminBusy} onclick={() => store.installAdmin()}>Install</Button>{/if}
    </div>
  {/each}
</section>
