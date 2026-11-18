<script lang="ts">
  // Ajustes → Atajos (`F-Ajustes--atajos`): grabar la combinación de cada atajo global con la
  // lógica de `shortcuts.ts` y guardarla en el núcleo de la app.
  import { onMount } from "svelte";
  import { useStore } from "../lib/context";
  import type { Bindings, ShortcutAction, ShortcutsView } from "../lib/model";
  import { ACTION_LABEL, ACTIONS, DEFAULT_BINDINGS, duplicateOf, formatAccel, isMacPlatform, readCombo } from "../lib/shortcuts";
  import Button from "../ui/Button.svelte";

  const store = useStore();
  const mac = isMacPlatform();

  let view: ShortcutsView | null = $state(null);
  let draft: Bindings = $state({ ...DEFAULT_BINDINGS });
  let recording: ShortcutAction | null = $state(null);
  let error: string | null = $state(null);
  let saving = $state(false);

  const dirty = $derived(view !== null && ACTIONS.some((a) => draft[a] !== view?.bindings[a]));

  onMount(async () => {
    try {
      view = await store.shortcuts();
      draft = { ...view.bindings };
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  });

  function record(a: ShortcutAction) {
    error = null;
    recording = recording === a ? null : a;
  }

  function onkeydown(e: KeyboardEvent, a: ShortcutAction) {
    if (recording !== a) return;
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      recording = null;
      return;
    }
    const c = readCombo(e);
    if (c.kind === "partial") return;
    if (c.kind === "invalid") {
      error = c.reason;
      return;
    }
    const dup = duplicateOf(draft, a, c.accel);
    if (dup) {
      error = `Ya lo usa «${ACTION_LABEL[dup]}»`;
      return;
    }
    draft = { ...draft, [a]: c.accel };
    error = null;
    recording = null;
  }

  async function save() {
    saving = true;
    try {
      view = await store.setShortcuts(draft);
      draft = { ...view.bindings };
      error = null;
      store.toast("Atajos guardados");
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }
</script>

{#if view?.warning}<p class="warn">{view.warning}</p>{/if}
<p class="mut intro">Funcionan en cualquier máquina del puesto, aunque Tandem esté en segundo plano. Haz clic y pulsa la combinación nueva.</p>
{#each ACTIONS as a (a)}
  <div class="row">
    <span class="fill col">
      {ACTION_LABEL[a]}
      {#if view?.failed[a]}<span class="bad">El sistema no lo dejó registrar: {view.failed[a]}. Funciona con la ventana al frente.</span>{/if}
    </span>
    <button
      type="button"
      class="combo"
      class:rec={recording === a}
      aria-label="Atajo de «{ACTION_LABEL[a]}»: {recording === a ? 'pulsa la combinación nueva' : formatAccel(draft[a], mac) || 'sin atajo'}"
      aria-pressed={recording === a}
      onclick={() => record(a)}
      onkeydown={(e) => onkeydown(e, a)}>{recording === a ? "Pulsa…" : formatAccel(draft[a], mac) || "—"}</button
    >
  </div>
{/each}
{#if error}<p class="bad" role="alert">{error}</p>{/if}
<div class="row end">
  <Button icon="refresh" variant="ghost" motion="spin" onclick={() => ((draft = { ...DEFAULT_BINDINGS }), (error = null))}>Restablecer</Button>
  <span class="fill"></span>
  <Button icon="check" variant="pri" disabled={!dirty || saving} onclick={save}>Guardar</Button>
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 0;
    border-top: 1px solid #1f2126;
    font-size: 13.5px;
    flex-wrap: wrap;
  }
  .intro {
    margin: 0 0 8px;
    font-size: 12.5px;
  }
  .row.end {
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
  /* El campo de la combinación (F-Ajustes). */
  .combo {
    font-family: "JetBrains Mono", monospace;
    font-size: 12.5px;
    padding: 6px 12px;
    border-radius: 8px;
    background: #111215;
    border: 1px solid #31343b;
    min-width: 110px;
    text-align: center;
    color: #e8e8eb;
    cursor: pointer;
    transition: border-color 0.2s;
  }
  .combo:hover {
    border-color: #4a4e58;
  }
  .combo.rec {
    border-style: dashed;
    border-color: #7aa7e6;
    animation: combo-bd 1.6s ease-in-out infinite;
  }
  .combo:focus-visible {
    outline: 2px solid #7aa7e6;
    outline-offset: 2px;
  }
  .bad {
    color: #f0b1aa;
    font-size: 12.5px;
    margin: 0;
  }
  .warn {
    color: #e6c48f;
    font-size: 12.5px;
    margin: 0 0 6px;
  }
  @keyframes combo-bd {
    50% {
      border-color: #4a4e58;
    }
  }
</style>
