<script lang="ts">
  // Paso 1 (`F-Asis1`): nombre y color de esta máquina y lo que Tandem encontró en ella.
  import { useStore } from "../../lib/context";
  import { PALETTE } from "../../lib/machine";
  import { foundRows } from "../../lib/wizard";

  const store = useStore();
  const here = $derived(store.desk.machines[store.desk.here]);
  const rows = $derived(foundRows(here ?? null));
  let draft = $state<string | null>(null);
  const name = $derived(draft ?? here?.name ?? "");

  /** El nombre se guarda al salir del campo o con Enter; vacío vuelve al que había. */
  async function commit() {
    const text = (draft ?? "").trim();
    draft = null;
    if (!here || !text || text === here.name) return;
    await store.editMachine(here.id, { name: text });
  }
</script>

<div class="wz-head">
  <span class="wz-mut wz-sm">Paso 1 de 6</span>
  <h1>Esta máquina</h1>
  <p class="wz-mut wz-lead">Tandem revisa qué sabe hacer esta máquina. Nada está fijo: lo que encuentra es lo que anuncia al resto del puesto.</p>
</div>
<div style="display:flex;gap:18px;flex-wrap:wrap;align-items:flex-end">
  <label style="display:flex;flex-direction:column;gap:6px;font-size:13px;flex:1 1 260px"
    >Nombre
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
<section aria-label="Lo que encontró" class="wz-list">
  <div class="wz-lt"><i style="background:{here?.color ?? '#7aa7e6'}"></i>Lo que encontró</div>
  {#if !here}
    <div class="wz-chk"><span class="wz-off"></span>Sin conexión con tandemd<span class="wz-fill"></span><span class="wz-mut wz-xs">{store.desk.troubleText}</span></div>
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
      {r.label}<span class="wz-fill"></span>{#if r.note}<span class="wz-mut wz-xs" class:wz-mono={r.mono}>{r.note}</span>{/if}
    </div>
  {/each}
</section>
